//! Outbound HTTP(S) for the image proxy and OIDC: one request, one connection, to an address
//! the caller has approved. The host is resolved here, every resolved address is checked, and
//! the connection goes to the checked address, so a DNS answer that changes between the check
//! and the connect (rebinding) cannot reach anything else. TLS is rustls with the bundled
//! webpki roots and is verified against the URL's host name.
//!
//! No cookies, no `Referer`, no proxies, no redirects (callers follow them, re-checking each hop).

use axum::body::Bytes;
use http_body_util::{BodyExt, Full};
use hyper_util::rt::TokioIo;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, OnceLock};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_rustls::rustls;
use url::{Host, Url};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FetchError {
    /// Not an http(s) URL, or one with credentials or no host.
    BadUrl,
    /// The host resolves to an address the policy refuses.
    Forbidden,
    /// The host does not resolve.
    Resolve,
    Connect,
    Tls,
    /// The answer is not HTTP/1.1 we can read.
    Protocol,
    TooLarge,
    Timeout,
}

/// Which addresses a request may reach.
pub type AddressPolicy = Arc<dyn Fn(IpAddr) -> bool + Send + Sync>;

pub struct Request { pub method: hyper::Method, pub url: Url, pub headers: Vec<(&'static str, String)>, pub body: Vec<u8> }

impl Request {
    pub fn get(url: Url) -> Self { Self { method: hyper::Method::GET, url, headers: Vec::new(), body: Vec::new() } }
}

pub struct Reply { pub status: u16, pub headers: hyper::HeaderMap, pub body: Vec<u8> }

impl Reply {
    pub fn header(&self, name: &str) -> Option<&str> { self.headers.get(name).and_then(|v| v.to_str().ok()) }
}

/// Check the URL's shape: http or https, a host, no `user:pass@`.
pub fn check_url(url: &Url) -> Result<(), FetchError> {
    if !matches!(url.scheme(), "http" | "https") || url.host().is_none() || !url.username().is_empty() || url.password().is_some() { return Err(FetchError::BadUrl); }
    Ok(())
}

/// Resolve the URL's host and return one address to connect to, if the policy allows every
/// address the host resolves to.
pub async fn resolve(url: &Url, allow: &AddressPolicy) -> Result<SocketAddr, FetchError> {
    check_url(url)?;
    let port = url.port_or_known_default().ok_or(FetchError::BadUrl)?;
    let addresses: Vec<SocketAddr> = match url.host().ok_or(FetchError::BadUrl)? {
        Host::Ipv4(ip) => vec![SocketAddr::new(ip.into(), port)],
        Host::Ipv6(ip) => vec![SocketAddr::new(ip.into(), port)],
        Host::Domain(domain) => tokio::net::lookup_host((domain, port)).await.map_err(|_| FetchError::Resolve)?.collect(),
    };
    let first = *addresses.first().ok_or(FetchError::Resolve)?;
    if addresses.iter().any(|a| !allow(a.ip())) { return Err(FetchError::Forbidden); }
    Ok(first)
}

/// Send one request to `address` (from [`resolve`]) and read at most `limit` body bytes.
pub async fn send(request: &Request, address: SocketAddr, limit: usize) -> Result<Reply, FetchError> {
    let stream = tokio::net::TcpStream::connect(address).await.map_err(|_| FetchError::Connect)?;
    if request.url.scheme() == "https" {
        let name = match request.url.host().ok_or(FetchError::BadUrl)? {
            Host::Domain(d) => rustls::pki_types::ServerName::try_from(d.to_owned()).map_err(|_| FetchError::BadUrl)?,
            Host::Ipv4(ip) => rustls::pki_types::ServerName::IpAddress(IpAddr::from(ip).into()),
            Host::Ipv6(ip) => rustls::pki_types::ServerName::IpAddress(IpAddr::from(ip).into()),
        };
        let tls = tokio_rustls::TlsConnector::from(tls_config()?).connect(name, stream).await.map_err(|_| FetchError::Tls)?;
        exchange(tls, request, limit).await
    } else {
        exchange(stream, request, limit).await
    }
}

fn tls_config() -> Result<Arc<rustls::ClientConfig>, FetchError> {
    static CONFIG: OnceLock<Option<Arc<rustls::ClientConfig>>> = OnceLock::new();
    CONFIG.get_or_init(|| {
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let config = rustls::ClientConfig::builder_with_provider(provider).with_safe_default_protocol_versions().ok()?.with_root_certificates(roots).with_no_client_auth();
        Some(Arc::new(config))
    }).clone().ok_or(FetchError::Tls)
}

async fn exchange<S: AsyncRead + AsyncWrite + Unpin + Send + 'static>(io: S, request: &Request, limit: usize) -> Result<Reply, FetchError> {
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(io)).await.map_err(|_| FetchError::Protocol)?;
    let driver = tokio::spawn(async move { let _ = connection.await; });
    let host = match request.url.port() { Some(port) => format!("{}:{port}", request.url.host_str().unwrap_or_default()), None => request.url.host_str().unwrap_or_default().to_owned() };
    let target = match request.url.query() { Some(q) => format!("{}?{q}", request.url.path()), None => request.url.path().to_owned() };
    let mut builder = hyper::Request::builder().method(request.method.clone()).uri(target).header(hyper::header::HOST, host).header(hyper::header::USER_AGENT, "Ravenglass-Relay");
    for (name, value) in &request.headers { builder = builder.header(*name, value); }
    let outgoing = builder.body(Full::new(Bytes::from(request.body.clone()))).map_err(|_| FetchError::BadUrl)?;
    let response = sender.send_request(outgoing).await.map_err(|_| FetchError::Protocol)?;
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let declared = headers.get(hyper::header::CONTENT_LENGTH).and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<usize>().ok());
    if declared.is_some_and(|n| n > limit) { driver.abort(); return Err(FetchError::TooLarge); }
    let mut body = response.into_body();
    let mut bytes = Vec::new();
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|_| FetchError::Protocol)?;
        if let Some(data) = frame.data_ref() {
            if bytes.len() + data.len() > limit { driver.abort(); return Err(FetchError::TooLarge); }
            bytes.extend_from_slice(data);
        }
    }
    driver.abort();
    Ok(Reply { status, headers, body: bytes })
}
