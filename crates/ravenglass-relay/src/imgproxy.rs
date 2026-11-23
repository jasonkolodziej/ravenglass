//! The image proxy (architecture §8): fetch a remote image the user allowed, so the browser
//! never contacts the sender's server and the sender never learns the reader's address.
//!
//! Server-side request forgery is the risk: a URL that points into the Relay's own network.
//! So only http and https are fetched, only from public unicast addresses ([`is_public`]), each
//! of at most [`MAX_REDIRECTS`] redirects is checked again, and the connection goes to the
//! address that was checked ([`crate::outbound`]). The fetch sends no cookies and no referrer,
//! reads at most [`MAX_BYTES`], gives up after [`TIMEOUT`], and accepts `image/*` answers only.

use crate::outbound::{self, AddressPolicy, FetchError, Request};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::Arc;
use std::time::Duration;
use url::Url;

pub const MAX_BYTES: usize = 10 << 20;
pub const MAX_REDIRECTS: usize = 3;
pub const TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, PartialEq, Eq)]
pub struct Image { pub content_type: String, pub bytes: Vec<u8> }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImgError {
    Fetch(FetchError),
    /// The upstream answered something other than 200 (after redirects).
    Status(u16),
    TooManyRedirects,
    NotAnImage,
}

impl From<FetchError> for ImgError {
    fn from(e: FetchError) -> Self { ImgError::Fetch(e) }
}

/// The policy for real use: public addresses only.
pub fn public_only() -> AddressPolicy { Arc::new(is_public) }

/// Whether an address is public unicast: not loopback, private (RFC 1918, ULA), link-local
/// (which holds cloud metadata at 169.254.169.254), CGNAT, multicast, broadcast, documentation,
/// benchmarking or otherwise reserved. IPv4 embedded in IPv6 (mapped, compatible, NAT64, 6to4)
/// is judged as the IPv4 address.
pub fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_public_v4(v4),
        IpAddr::V6(v6) => is_public_v6(v6),
    }
}

fn is_public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(a == 0                                  // "this network"
        || a == 10                            // private
        || a == 127                           // loopback
        || (a == 100 && (64..128).contains(&b)) // CGNAT 100.64/10
        || (a == 169 && b == 254)             // link-local, cloud metadata
        || (a == 172 && (16..32).contains(&b)) // private
        || (a == 192 && b == 0 && c == 0)     // IETF protocol assignments
        || (a == 192 && b == 0 && c == 2)     // TEST-NET-1
        || (a == 192 && b == 88 && c == 99)   // 6to4 relay anycast
        || (a == 192 && b == 168)             // private
        || (a == 198 && (18..20).contains(&b)) // benchmarking
        || (a == 198 && b == 51 && c == 100)  // TEST-NET-2
        || (a == 203 && b == 0 && c == 113)   // TEST-NET-3
        || a >= 224)                          // multicast, reserved, broadcast
}

fn is_public_v6(ip: Ipv6Addr) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() { return is_public_v4(v4); }
    let s = ip.segments();
    let embedded = |hi: u16, lo: u16| Ipv4Addr::new((hi >> 8) as u8, hi as u8, (lo >> 8) as u8, lo as u8);
    if s[..6] == [0; 6] { return false; } // ::, ::1 and IPv4-compatible
    if s[0] == 0x64 && s[1] == 0xff9b { return is_public_v4(embedded(s[6], s[7])); } // NAT64
    if s[0] == 0x2002 { return is_public_v4(embedded(s[1], s[2])); } // 6to4
    !((s[0] & 0xfe00) == 0xfc00                // unique local, includes fd00:ec2::254
        || (s[0] & 0xffc0) == 0xfe80           // link-local
        || (s[0] & 0xffc0) == 0xfec0           // site-local (deprecated)
        || (s[0] & 0xff00) == 0xff00           // multicast
        || (s[0] == 0x2001 && s[1] == 0x0db8)  // documentation
        || (s[0] == 0x2001 && s[1] < 0x0200)   // IETF special purpose, Teredo
        || (s[0] == 0x0100 && s[1..4] == [0; 3])) // discard-only
}

/// Fetch an image, following and re-checking redirects, within [`TIMEOUT`].
pub async fn fetch(url: &str, allow: &AddressPolicy) -> Result<Image, ImgError> {
    let url = Url::parse(url).map_err(|_| FetchError::BadUrl)?;
    tokio::time::timeout(TIMEOUT, follow(url, allow)).await.unwrap_or(Err(ImgError::Fetch(FetchError::Timeout)))
}

async fn follow(mut url: Url, allow: &AddressPolicy) -> Result<Image, ImgError> {
    for _ in 0..=MAX_REDIRECTS {
        let address = outbound::resolve(&url, allow).await?;
        let mut request = Request::get(url.clone());
        request.headers.push(("accept", "image/*".into()));
        let reply = outbound::send(&request, address, MAX_BYTES).await?;
        if matches!(reply.status, 301 | 302 | 303 | 307 | 308) {
            let location = reply.header("location").ok_or(ImgError::Status(reply.status))?;
            url = url.join(location).map_err(|_| FetchError::BadUrl)?;
            continue;
        }
        if reply.status != 200 { return Err(ImgError::Status(reply.status)); }
        let content_type = reply.header("content-type").unwrap_or_default().trim().to_ascii_lowercase();
        let essence = content_type.split(';').next().unwrap_or_default().trim().to_owned();
        if !essence.starts_with("image/") || essence.len() <= "image/".len() { return Err(ImgError::NotAnImage); }
        return Ok(Image { content_type: essence, bytes: reply.body });
    }
    Err(ImgError::TooManyRedirects)
}

#[cfg(test)]
mod tests;
