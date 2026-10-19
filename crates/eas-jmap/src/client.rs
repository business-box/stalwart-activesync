//! Authenticated JMAP calls on behalf of one user.
//!
//! A [`JmapClient`] wraps a [`Session`] with the credentials that opened it.
//! Every HTTP request first takes a permit from the user's semaphore, sized
//! from the session's `maxConcurrentRequests`, so one phone syncing many
//! folders cannot trip Stalwart's per-user limit.

use crate::session::{Credentials, JmapError, Session, StalwartEndpoint};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex, Weak};
use tokio::sync::Semaphore;

/// One method call in a batch: name, arguments and the call ID used for
/// back-references (`#ids` with `resultOf`).
#[derive(Debug, Clone)]
pub struct Call {
    pub method: String,
    pub args: Value,
    pub id: String,
}

impl Call {
    pub fn new(method: &str, args: Value, id: impl Into<String>) -> Call {
        Call {
            method: method.to_string(),
            args,
            id: id.into(),
        }
    }
}

/// Semaphores per user, shared by all of that user's devices.
#[derive(Default)]
pub struct UserLimits {
    map: Mutex<HashMap<String, Weak<Semaphore>>>,
}

impl UserLimits {
    pub fn new() -> UserLimits {
        UserLimits::default()
    }

    /// The user's semaphore; created with `permits` if no request of that
    /// user is in flight.
    pub fn get(&self, username: &str, permits: u32) -> Arc<Semaphore> {
        let mut map = self.map.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(sem) = map.get(username).and_then(Weak::upgrade) {
            return sem;
        }
        if map.len() > 1_000 {
            map.retain(|_, w| w.strong_count() > 0);
        }
        let sem = Arc::new(Semaphore::new(permits.max(1) as usize));
        map.insert(username.to_string(), Arc::downgrade(&sem));
        sem
    }
}

#[derive(Clone)]
pub struct JmapClient {
    endpoint: StalwartEndpoint,
    session: Session,
    creds: Credentials,
    client_ip: Option<IpAddr>,
    limit: Arc<Semaphore>,
}

const USING: [&str; 3] = [
    crate::capability::CORE,
    crate::capability::MAIL,
    crate::capability::SUBMISSION,
];

impl JmapClient {
    pub fn new(
        endpoint: StalwartEndpoint,
        session: Session,
        creds: Credentials,
        client_ip: Option<IpAddr>,
        limits: &UserLimits,
    ) -> JmapClient {
        let limit = limits.get(&session.username, session.max_concurrent_requests);
        JmapClient {
            endpoint,
            session,
            creds,
            client_ip,
            limit,
        }
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    /// The account mail calls run against.
    pub fn account(&self) -> &str {
        &self.session.mail_account
    }

    fn request(&self, method: reqwest::Method, url: &str) -> reqwest::RequestBuilder {
        let mut req = self
            .endpoint
            .http()
            .request(method, url)
            .basic_auth(&self.creds.username, Some(&self.creds.password));
        if let Some(ip) = self.client_ip {
            req = req.header("X-Forwarded-For", ip.to_string());
        }
        req
    }

    async fn send(&self, req: reqwest::RequestBuilder) -> Result<reqwest::Response, JmapError> {
        let _permit = self
            .limit
            .acquire()
            .await
            .map_err(|_| JmapError::Protocol("concurrency limiter closed".into()))?;
        let resp = req.send().await?;
        match resp.status().as_u16() {
            200..=299 => Ok(resp),
            401 | 403 => Err(JmapError::Unauthorized),
            429 => Err(JmapError::Throttled),
            s => Err(JmapError::Status(s)),
        }
    }

    /// Runs `calls` in one JMAP request and returns each call's arguments in
    /// order. A method-level error in any call fails the whole batch, since
    /// later calls usually depend on earlier ones.
    pub async fn batch(&self, calls: Vec<Call>) -> Result<Vec<Value>, JmapError> {
        let mut out = Vec::with_capacity(calls.len());
        let max = self.session.max_calls_in_request as usize;
        // Chunks only split batches without back-references; callers that use
        // `#` references keep their batches below the limit.
        for chunk in calls.chunks(max.max(1)) {
            let method_calls: Vec<Value> = chunk
                .iter()
                .map(|c| json!([c.method, c.args, c.id]))
                .collect();
            let body = json!({ "using": USING, "methodCalls": method_calls });
            let resp = self
                .send(
                    self.request(reqwest::Method::POST, &self.session.api_url)
                        .json(&body),
                )
                .await?;
            let value: Value = resp.json().await?;
            out.extend(parse_responses(chunk, value)?);
        }
        Ok(out)
    }

    /// A single method call.
    pub async fn call(&self, method: &str, args: Value) -> Result<Value, JmapError> {
        let mut res = self.batch(vec![Call::new(method, args, "c0")]).await?;
        res.pop()
            .ok_or_else(|| JmapError::Protocol("empty methodResponses".into()))
    }

    /// Downloads a blob, such as a full RFC 5322 message or an attachment.
    pub async fn download(&self, blob_id: &str, name: &str) -> Result<Vec<u8>, JmapError> {
        let url = expand_template(
            &self.session.download_url,
            &[
                ("accountId", self.account()),
                ("blobId", blob_id),
                ("name", name),
                ("type", "application/octet-stream"),
            ],
        );
        let resp = self.send(self.request(reqwest::Method::GET, &url)).await?;
        Ok(resp.bytes().await?.to_vec())
    }

    /// Uploads a blob and returns its ID.
    pub async fn upload(&self, data: Vec<u8>, content_type: &str) -> Result<String, JmapError> {
        let url = expand_template(&self.session.upload_url, &[("accountId", self.account())]);
        let resp = self
            .send(
                self.request(reqwest::Method::POST, &url)
                    .header("Content-Type", content_type)
                    .body(data),
            )
            .await?;
        let v: Value = resp.json().await?;
        v.get("blobId")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| JmapError::Protocol("upload response without blobId".into()))
    }
}

fn parse_responses(calls: &[Call], value: Value) -> Result<Vec<Value>, JmapError> {
    let responses = value
        .get("methodResponses")
        .and_then(Value::as_array)
        .ok_or_else(|| JmapError::Protocol("response without methodResponses".into()))?;
    let mut out = Vec::with_capacity(calls.len());
    // A call may produce several responses (implicit calls such as
    // onSuccessUpdateEmail); keep the first one per call ID.
    for call in calls {
        let resp = responses
            .iter()
            .find(|r| r.get(2).and_then(Value::as_str) == Some(call.id.as_str()))
            .ok_or_else(|| JmapError::Protocol(format!("no response for {}", call.id)))?;
        let name = resp.get(0).and_then(Value::as_str).unwrap_or_default();
        let args = resp.get(1).cloned().unwrap_or(Value::Null);
        if name == "error" {
            return Err(JmapError::Method {
                method: call.method.clone(),
                kind: args
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("serverFail")
                    .to_string(),
                description: args
                    .get("description")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            });
        }
        out.push(args);
    }
    Ok(out)
}

/// Fills an RFC 6570 level-1 URI template as used by the JMAP session's
/// download and upload URLs.
fn expand_template(template: &str, vars: &[(&str, &str)]) -> String {
    let mut out = template.to_string();
    for (k, v) in vars {
        let enc: String = url::form_urlencoded::byte_serialize(v.as_bytes())
            .collect::<String>()
            .replace('+', "%20");
        out = out.replace(&format!("{{{k}}}"), &enc);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_is_percent_encoded() {
        let t = "https://x/jmap/download/{accountId}/{blobId}/{name}?accept={type}";
        assert_eq!(
            expand_template(
                t,
                &[
                    ("accountId", "a"),
                    ("blobId", "b1"),
                    ("name", "my file.eml"),
                    ("type", "application/octet-stream")
                ]
            ),
            "https://x/jmap/download/a/b1/my%20file.eml?accept=application%2Foctet-stream"
        );
    }

    #[test]
    fn method_errors_and_order() {
        let calls = vec![
            Call::new("Email/changes", json!({}), "a"),
            Call::new("Email/get", json!({}), "b"),
        ];
        let ok = json!({"methodResponses": [
            ["Email/get", {"list": []}, "b"],
            ["Email/changes", {"newState": "2"}, "a"]
        ]});
        let res = parse_responses(&calls, ok).unwrap();
        assert_eq!(res[0]["newState"], "2");
        assert!(res[1]["list"].is_array());

        let err = json!({"methodResponses": [
            ["error", {"type": "cannotCalculateChanges"}, "a"],
            ["Email/get", {"list": []}, "b"]
        ]});
        match parse_responses(&calls, err) {
            Err(JmapError::Method { kind, .. }) => assert_eq!(kind, "cannotCalculateChanges"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn limits_are_shared_per_user() {
        let l = UserLimits::new();
        let a = l.get("alice", 2);
        let b = l.get("alice", 8);
        assert!(Arc::ptr_eq(&a, &b));
        assert_eq!(b.available_permits(), 2);
        drop((a, b));
        assert_eq!(l.get("alice", 8).available_permits(), 8);
    }
}
