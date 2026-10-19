use serde::Deserialize;
use std::collections::HashMap;
use std::fmt;
use std::net::IpAddr;
use std::time::Duration;

/// Capability URNs the gateway relies on.
pub mod capability {
    pub const CORE: &str = "urn:ietf:params:jmap:core";
    pub const MAIL: &str = "urn:ietf:params:jmap:mail";
    pub const SUBMISSION: &str = "urn:ietf:params:jmap:submission";
    pub const CALENDARS: &str = "urn:ietf:params:jmap:calendars";
    pub const CONTACTS: &str = "urn:ietf:params:jmap:contacts";
    pub const VACATION: &str = "urn:ietf:params:jmap:vacationresponse";
}

/// Basic credentials as the device sent them. `Debug` never prints the
/// password.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Credentials {
    pub username: String,
    pub password: String,
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .finish()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum JmapError {
    #[error("Stalwart rejected the credentials")]
    Unauthorized,
    #[error("Stalwart is rate limiting or banning this client")]
    Throttled,
    #[error("Stalwart returned HTTP {0}")]
    Status(u16),
    #[error("JMAP session is missing the {0} capability")]
    MissingCapability(&'static str),
    #[error("cannot reach Stalwart: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("invalid Stalwart URL: {0}")]
    Url(#[from] url::ParseError),
    #[error("{method} failed: {kind}")]
    Method {
        method: String,
        kind: String,
        description: Option<String>,
    },
    #[error("unexpected JMAP response: {0}")]
    Protocol(String),
}

impl JmapError {
    /// True for a method error of the given JMAP type, such as
    /// `cannotCalculateChanges`.
    pub fn is_method(&self, t: &str) -> bool {
        matches!(self, JmapError::Method { kind, .. } if kind == t)
    }
}

/// Where and how to reach Stalwart's JMAP API.
#[derive(Clone)]
pub struct StalwartEndpoint {
    base: url::Url,
    http: reqwest::Client,
    rewrite_urls: bool,
}

impl StalwartEndpoint {
    /// `base` is Stalwart's HTTP root, for example `http://stalwart:8080/`.
    pub fn new(base: &str) -> Result<StalwartEndpoint, JmapError> {
        let mut base = url::Url::parse(base)?;
        if !base.path().ends_with('/') {
            base.set_path(&format!("{}/", base.path()));
        }
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(60))
            .user_agent(concat!("stalwart-activesync/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(StalwartEndpoint {
            base,
            http,
            rewrite_urls: true,
        })
    }

    /// Whether the session's API, download, upload and EventSource URLs
    /// are moved onto the base URL's origin (on by default). Stalwart
    /// advertises its public `https://` hostname, which the gateway often
    /// can't reach or doesn't trust from inside a private network.
    pub fn with_url_rewrite(mut self, on: bool) -> StalwartEndpoint {
        self.rewrite_urls = on;
        self
    }

    pub fn base_url(&self) -> &url::Url {
        &self.base
    }

    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    /// Opens the user's JMAP session, which also checks the credentials.
    /// `client_ip` is forwarded as `X-Forwarded-For` so that Stalwart's
    /// brute-force protection blocks the phone, not the gateway (requires
    /// Stalwart's forwarded-header handling to trust the gateway).
    pub async fn session(
        &self,
        creds: &Credentials,
        client_ip: Option<IpAddr>,
    ) -> Result<Session, JmapError> {
        let url = self.base.join(".well-known/jmap")?;
        let mut req = self
            .http
            .get(url)
            .basic_auth(&creds.username, Some(&creds.password));
        if let Some(ip) = client_ip {
            req = req.header("X-Forwarded-For", ip.to_string());
        }
        let resp = req.send().await?;
        match resp.status().as_u16() {
            200 => {}
            401 | 403 => return Err(JmapError::Unauthorized),
            429 => return Err(JmapError::Throttled),
            s => return Err(JmapError::Status(s)),
        }
        let raw: RawSession = resp.json().await?;
        let mut session = Session::from_raw(raw)?;
        if self.rewrite_urls {
            let origin = self.base.origin().ascii_serialization();
            for u in [
                &mut session.api_url,
                &mut session.download_url,
                &mut session.upload_url,
                &mut session.event_source_url,
            ] {
                *u = rebase(u, &origin);
            }
        }
        Ok(session)
    }
}

/// Replaces the scheme and authority of `url` with `origin`, keeping the
/// path and query. Works on the raw string, so URI template braces such as
/// `{accountId}` survive (a URL parser would percent-encode them).
fn rebase(url: &str, origin: &str) -> String {
    let Some((_, rest)) = url.split_once("://") else {
        return url.to_string();
    };
    match rest.find('/') {
        Some(i) => format!("{origin}{}", &rest[i..]),
        None => origin.to_string(),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawSession {
    username: String,
    api_url: String,
    download_url: String,
    upload_url: String,
    event_source_url: String,
    #[serde(default)]
    primary_accounts: HashMap<String, String>,
    capabilities: HashMap<String, serde_json::Value>,
    state: String,
}

/// The parts of a JMAP session (RFC 8620, section 2) the gateway uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub username: String,
    pub api_url: String,
    pub download_url: String,
    pub upload_url: String,
    pub event_source_url: String,
    /// Primary account for mail; also used for calendars and contacts unless
    /// those capabilities name another one.
    pub mail_account: String,
    pub calendar_account: Option<String>,
    pub contacts_account: Option<String>,
    pub max_concurrent_requests: u32,
    pub max_calls_in_request: u32,
    pub max_size_upload: u64,
    pub state: String,
    /// Capability URNs the server offers, sorted.
    pub capabilities: Vec<String>,
}

impl Session {
    pub fn has(&self, capability: &str) -> bool {
        self.capabilities.iter().any(|c| c == capability)
    }

    /// Account for contacts, if the server offers them.
    pub fn contacts(&self) -> Option<&str> {
        self.has(capability::CONTACTS).then(|| {
            self.contacts_account
                .as_deref()
                .unwrap_or(&self.mail_account)
        })
    }

    /// Account for calendars, if the server offers them.
    pub fn calendars(&self) -> Option<&str> {
        self.has(capability::CALENDARS).then(|| {
            self.calendar_account
                .as_deref()
                .unwrap_or(&self.mail_account)
        })
    }
}

impl Session {
    fn from_raw(raw: RawSession) -> Result<Session, JmapError> {
        let core = raw
            .capabilities
            .get(capability::CORE)
            .ok_or(JmapError::MissingCapability(capability::CORE))?;
        let num = |k: &str, default: u64| core.get(k).and_then(|v| v.as_u64()).unwrap_or(default);
        let mail_account = raw
            .primary_accounts
            .get(capability::MAIL)
            .cloned()
            .ok_or(JmapError::MissingCapability(capability::MAIL))?;
        Ok(Session {
            username: raw.username,
            api_url: raw.api_url,
            download_url: raw.download_url,
            upload_url: raw.upload_url,
            event_source_url: raw.event_source_url,
            calendar_account: raw.primary_accounts.get(capability::CALENDARS).cloned(),
            contacts_account: raw.primary_accounts.get(capability::CONTACTS).cloned(),
            mail_account,
            max_concurrent_requests: num("maxConcurrentRequests", 4).clamp(1, 64) as u32,
            max_calls_in_request: num("maxCallsInRequest", 16).clamp(1, 1024) as u32,
            max_size_upload: num("maxSizeUpload", 50_000_000),
            state: raw.state,
            capabilities: {
                let mut c: Vec<String> = raw.capabilities.keys().cloned().collect();
                c.sort();
                c
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_stalwart_style_session() {
        let json = r#"{
            "capabilities": {
                "urn:ietf:params:jmap:core": {"maxSizeUpload": 50000000, "maxConcurrentRequests": 8, "maxCallsInRequest": 32},
                "urn:ietf:params:jmap:mail": {},
                "urn:ietf:params:jmap:calendars": {}
            },
            "accounts": {},
            "primaryAccounts": {
                "urn:ietf:params:jmap:mail": "b",
                "urn:ietf:params:jmap:calendars": "b"
            },
            "username": "alice@example.org",
            "apiUrl": "https://mail.example.org/jmap/",
            "downloadUrl": "https://mail.example.org/jmap/download/{accountId}/{blobId}/{name}?accept={type}",
            "uploadUrl": "https://mail.example.org/jmap/upload/{accountId}/",
            "eventSourceUrl": "https://mail.example.org/jmap/eventsource/?types={types}&closeafter={closeafter}&ping={ping}",
            "state": "s1"
        }"#;
        let s = Session::from_raw(serde_json::from_str(json).unwrap()).unwrap();
        assert_eq!(s.mail_account, "b");
        assert_eq!(s.calendar_account.as_deref(), Some("b"));
        assert_eq!(s.contacts_account, None);
        assert_eq!(s.calendars(), Some("b"));
        assert_eq!(s.contacts(), None);
        assert_eq!(s.max_concurrent_requests, 8);
        assert_eq!(s.max_calls_in_request, 32);
    }

    #[test]
    fn session_urls_move_to_the_base_origin() {
        assert_eq!(
            rebase(
                "https://mail.example.org/jmap/download/{accountId}/{blobId}/{name}?accept={type}",
                "http://stalwart:8080"
            ),
            "http://stalwart:8080/jmap/download/{accountId}/{blobId}/{name}?accept={type}"
        );
        assert_eq!(
            rebase("https://mail.example.org", "http://s:1"),
            "http://s:1"
        );
        assert_eq!(rebase("relative/path", "http://s:1"), "relative/path");
    }

    #[test]
    fn credentials_debug_hides_password() {
        let c = Credentials {
            username: "a".into(),
            password: "hunter2".into(),
        };
        assert!(!format!("{c:?}").contains("hunter2"));
    }

    #[test]
    fn base_url_gets_trailing_slash() {
        let e = StalwartEndpoint::new("http://stalwart:8080/sub").unwrap();
        assert_eq!(e.base_url().as_str(), "http://stalwart:8080/sub/");
        assert_eq!(
            e.base_url().join(".well-known/jmap").unwrap().as_str(),
            "http://stalwart:8080/sub/.well-known/jmap"
        );
    }
}
