//! JMAP push over EventSource (RFC 8620, section 7.3).
//!
//! The stream carries `state` events (a `StateChange` naming the data types
//! whose state moved, per account) and `ping` keep-alives. Only the event
//! name and the `changed` map are parsed; everything else is ignored.

use crate::client::JmapClient;
use crate::session::JmapError;
use serde::Deserialize;
use std::collections::HashMap;
use std::time::Duration;

/// One event from the stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PushEvent {
    /// Types whose state changed, per account ID: `{account: {type: state}}`.
    State(HashMap<String, HashMap<String, String>>),
    Ping,
}

#[derive(Deserialize)]
struct StateChange {
    #[serde(default)]
    changed: HashMap<String, HashMap<String, String>>,
}

/// Incremental parser for `text/event-stream`.
#[derive(Default)]
pub struct SseParser {
    buf: Vec<u8>,
    event: String,
    data: String,
}

/// Longest line accepted; anything longer is a broken or hostile stream.
const MAX_LINE: usize = 1 << 20;

impl SseParser {
    /// Feeds bytes and returns the events they complete.
    pub fn feed(&mut self, bytes: &[u8]) -> Result<Vec<PushEvent>, JmapError> {
        self.buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        while let Some(nl) = self.buf.iter().position(|b| *b == b'\n') {
            let mut line: Vec<u8> = self.buf.drain(..=nl).collect();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            let line = String::from_utf8_lossy(&line);
            if line.is_empty() {
                if let Some(ev) = self.dispatch() {
                    out.push(ev);
                }
                continue;
            }
            if line.starts_with(':') {
                continue;
            }
            let (field, value) = match line.split_once(':') {
                Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
                None => (line.as_ref(), ""),
            };
            match field {
                "event" => self.event = value.to_string(),
                "data" => {
                    if !self.data.is_empty() {
                        self.data.push('\n');
                    }
                    self.data.push_str(value);
                }
                _ => {}
            }
        }
        if self.buf.len() > MAX_LINE || self.data.len() > MAX_LINE {
            return Err(JmapError::Protocol("event stream line too long".into()));
        }
        Ok(out)
    }

    fn dispatch(&mut self) -> Option<PushEvent> {
        let event = std::mem::take(&mut self.event);
        let data = std::mem::take(&mut self.data);
        match event.as_str() {
            "ping" => Some(PushEvent::Ping),
            "state" | "" if !data.is_empty() => serde_json::from_str::<StateChange>(&data)
                .ok()
                .map(|s| PushEvent::State(s.changed)),
            _ => None,
        }
    }
}

/// An open EventSource connection.
pub struct EventSource {
    resp: reqwest::Response,
    parser: SseParser,
    pending: std::collections::VecDeque<PushEvent>,
}

impl EventSource {
    /// The next event; `None` when the server closed the stream.
    pub async fn next(&mut self) -> Result<Option<PushEvent>, JmapError> {
        loop {
            if let Some(ev) = self.pending.pop_front() {
                return Ok(Some(ev));
            }
            match self.resp.chunk().await? {
                Some(bytes) => self.pending.extend(self.parser.feed(&bytes)?),
                None => return Ok(None),
            }
        }
    }
}

impl JmapClient {
    /// Opens the EventSource for all types, asking for a ping every
    /// `ping` and closing after `max_age` so a stale connection never lives
    /// forever. The stream takes no concurrency permit: it is idle almost
    /// all of the time.
    pub async fn event_source(
        &self,
        ping: Duration,
        max_age: Duration,
    ) -> Result<EventSource, JmapError> {
        let url = crate::client::expand_template(
            &self.session().event_source_url,
            &[
                ("types", "*"),
                ("closeafter", "no"),
                ("ping", &ping.as_secs().to_string()),
            ],
        );
        let resp = self
            .request(reqwest::Method::GET, &url)
            .header("Accept", "text/event-stream")
            .timeout(max_age)
            .send()
            .await?;
        match resp.status().as_u16() {
            200..=299 => {}
            401 | 403 => return Err(JmapError::Unauthorized),
            429 => return Err(JmapError::Throttled),
            s => return Err(JmapError::Status(s)),
        }
        Ok(EventSource {
            resp,
            parser: SseParser::default(),
            pending: Default::default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_state_and_ping_across_chunks() {
        let mut p = SseParser::default();
        assert!(p
            .feed(b": comment\r\nevent: state\r\ndata: {\"@type\":\"StateChange\",")
            .unwrap()
            .is_empty());
        let evs = p
            .feed(b"\"changed\":{\"a\":{\"Email\":\"s2\"}}}\r\n\r\nevent: ping\ndata: {\"interval\":60}\n\n")
            .unwrap();
        assert_eq!(evs.len(), 2);
        match &evs[0] {
            PushEvent::State(c) => assert_eq!(c["a"]["Email"], "s2"),
            other => panic!("{other:?}"),
        }
        assert_eq!(evs[1], PushEvent::Ping);
    }

    #[test]
    fn ignores_garbage_and_bounds_lines() {
        let mut p = SseParser::default();
        assert!(p
            .feed(b"event: state\ndata: not json\n\n")
            .unwrap()
            .is_empty());
        assert!(p.feed(b"event: other\ndata: {}\n\n").unwrap().is_empty());
        assert!(p.feed(&vec![b'x'; MAX_LINE + 1]).is_err());
    }
}
