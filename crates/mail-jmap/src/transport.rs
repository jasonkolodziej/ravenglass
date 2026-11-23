//! A blocking HTTPS transport for native builds. Each request blocks the calling thread, which
//! is the engine thread, not the UI.

use crate::JmapHttp;
use mail_core::{BoxFut, MailError, Result};

/// Responses larger than this are refused (a 50 MB message plus headroom).
const MAX_RESPONSE: u64 = 64 << 20;

pub enum Credential {
    /// User name and password or app password (HTTP Basic).
    Basic { username: String, password: String },
    /// An OAuth access token or API token.
    Bearer(String),
}

pub struct HttpsTransport { authorization: String, agent: ureq::Agent }

impl HttpsTransport {
    pub fn new(credential: &Credential) -> Self {
        use base64::Engine as _;
        let authorization = match credential {
            Credential::Basic { username, password } => format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(format!("{username}:{password}"))),
            Credential::Bearer(token) => format!("Bearer {token}"),
        };
        let config = ureq::Agent::config_builder().http_status_as_error(false).https_only(true).timeout_global(Some(std::time::Duration::from_secs(60))).build();
        Self { authorization, agent: ureq::Agent::new_with_config(config) }
    }

    fn finish(sent: std::result::Result<ureq::http::Response<ureq::Body>, ureq::Error>) -> Result<Vec<u8>> {
        let mut response = sent.map_err(|e| MailError::Network(e.to_string()))?;
        match response.status().as_u16() {
            200..=299 => response.body_mut().with_config().limit(MAX_RESPONSE).read_to_vec().map_err(|e| MailError::Network(e.to_string())),
            401 | 403 => Err(MailError::Auth),
            404 => Err(MailError::NotFound("JMAP resource".into())),
            status => Err(MailError::Server(format!("HTTP {status}"))),
        }
    }
}

impl JmapHttp for HttpsTransport {
    fn get<'a>(&'a self, url: &'a str) -> BoxFut<'a, Result<Vec<u8>>> {
        Box::pin(async move { Self::finish(self.agent.get(url).header("authorization", &self.authorization).call()) })
    }

    fn post<'a>(&'a self, url: &'a str, content_type: &'a str, body: Vec<u8>) -> BoxFut<'a, Result<Vec<u8>>> {
        Box::pin(async move { Self::finish(self.agent.post(url).header("authorization", &self.authorization).header("content-type", content_type).send(&body[..])) })
    }
}
