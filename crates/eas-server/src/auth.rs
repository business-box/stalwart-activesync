//! Basic authentication against Stalwart, with a short-lived cache.
//!
//! EAS clients send Basic credentials on every request, and a `Ping` loop
//! means dozens of requests per device per hour. Opening a JMAP session for
//! each would hammer Stalwart, so successful logins are cached for a few
//! minutes, keyed by a hash of username and password. Failures are never
//! cached: Stalwart stays in charge of lockouts.

use axum::http::{header, HeaderMap};
use base64::Engine as _;
use eas_jmap::{Credentials, JmapError, Session, StalwartEndpoint};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Extracts Basic credentials from the `Authorization` header.
pub fn basic_credentials(headers: &HeaderMap) -> Option<Credentials> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, rest) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("basic") {
        return None;
    }
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(rest.trim())
        .ok()?;
    let decoded = String::from_utf8(decoded).ok()?;
    let (user, pass) = decoded.split_once(':')?;
    if user.is_empty() {
        return None;
    }
    // Some clients send DOMAIN\user; Stalwart logs in by address or name.
    let user = user.rsplit('\\').next().unwrap_or(user);
    Some(Credentials {
        username: user.to_string(),
        password: pass.to_string(),
    })
}

pub struct Authenticator {
    endpoint: StalwartEndpoint,
    ttl: Duration,
    cache: Mutex<HashMap<[u8; 32], (Session, Instant)>>,
}

impl Authenticator {
    pub fn new(endpoint: StalwartEndpoint, ttl: Duration) -> Authenticator {
        Authenticator {
            endpoint,
            ttl,
            cache: Mutex::new(HashMap::new()),
        }
    }

    pub fn endpoint(&self) -> &StalwartEndpoint {
        &self.endpoint
    }

    fn key(creds: &Credentials) -> [u8; 32] {
        let mut h = Sha256::new();
        h.update(creds.username.as_bytes());
        h.update([0u8]);
        h.update(creds.password.as_bytes());
        h.finalize().into()
    }

    /// Returns the user's JMAP session, from cache or from Stalwart.
    pub async fn authenticate(
        &self,
        creds: &Credentials,
        client_ip: Option<IpAddr>,
    ) -> Result<Session, JmapError> {
        let key = Self::key(creds);
        let now = Instant::now();
        if let Some((session, at)) = self.lock().get(&key) {
            if now.duration_since(*at) < self.ttl {
                return Ok(session.clone());
            }
        }
        let session = self.endpoint.session(creds, client_ip).await?;
        let mut cache = self.lock();
        if cache.len() > 10_000 {
            cache.retain(|_, (_, at)| now.duration_since(*at) < self.ttl);
        }
        cache.insert(key, (session.clone(), now));
        Ok(session)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<[u8; 32], (Session, Instant)>> {
        // A poisoned cache is still a valid cache.
        self.cache.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(v: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(header::AUTHORIZATION, HeaderValue::from_str(v).unwrap());
        h
    }

    #[test]
    fn parses_basic() {
        // "alice@example.org:pa:ss"
        let c = basic_credentials(&headers("Basic YWxpY2VAZXhhbXBsZS5vcmc6cGE6c3M=")).unwrap();
        assert_eq!(c.username, "alice@example.org");
        assert_eq!(c.password, "pa:ss");
    }

    #[test]
    fn strips_domain_prefix() {
        let enc = base64::engine::general_purpose::STANDARD.encode("EXAMPLE\\bob:x");
        let c = basic_credentials(&headers(&format!("basic {enc}"))).unwrap();
        assert_eq!(c.username, "bob");
    }

    #[test]
    fn rejects_other_schemes_and_garbage() {
        assert!(basic_credentials(&headers("Bearer abc")).is_none());
        assert!(basic_credentials(&headers("Basic !!!")).is_none());
        assert!(basic_credentials(&HeaderMap::new()).is_none());
        let enc = base64::engine::general_purpose::STANDARD.encode(":nouser");
        assert!(basic_credentials(&headers(&format!("Basic {enc}"))).is_none());
    }
}
