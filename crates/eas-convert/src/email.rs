//! JMAP `Email` objects to EAS email items (MS-ASEMAIL) and device changes
//! back to JMAP patches.

use crate::text::{html_to_text, text_to_html, truncate_utf8};
use chrono::{DateTime, Utc};
use eas_wbxml::{Element, Page};
use serde_json::{json, Map, Value};

/// JMAP properties [`application_data`] reads.
pub const PROPERTIES: &[&str] = &[
    "id",
    "blobId",
    "threadId",
    "keywords",
    "size",
    "receivedAt",
    "from",
    "to",
    "cc",
    "replyTo",
    "subject",
    "preview",
    "textBody",
    "htmlBody",
    "attachments",
    "bodyValues",
];

/// Properties [`state_hash`] reads.
pub const HASH_PROPERTIES: &[&str] = &["id", "keywords"];

/// Body formats (AirSyncBase `Type`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyType {
    Plain = 1,
    Html = 2,
    Mime = 4,
}

impl BodyType {
    pub fn from_code(code: u8) -> Option<BodyType> {
        match code {
            1 => Some(BodyType::Plain),
            2 => Some(BodyType::Html),
            4 => Some(BodyType::Mime),
            _ => None,
        }
    }
}

/// One `BodyPreference` from the request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BodyPreference {
    pub body_type: BodyType,
    pub truncation_size: Option<usize>,
    pub all_or_none: bool,
    pub preview: Option<usize>,
}

/// How the device wants bodies.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BodyOptions {
    pub preferences: Vec<BodyPreference>,
    /// `MIMESupport`: 0 never, 1 S/MIME only, 2 always.
    pub mime_support: u8,
    /// `MIMETruncation` in bytes.
    pub mime_truncation: Option<usize>,
}

impl BodyOptions {
    /// The preference the server answers with. MIME is used when the device
    /// asks for MIME on every message or offers nothing else; otherwise HTML
    /// beats plain text.
    pub fn chosen(&self) -> BodyPreference {
        let find = |t| self.preferences.iter().find(|p| p.body_type == t);
        if self.mime_support == 2 || (self.preferences.len() == 1 && find(BodyType::Mime).is_some())
        {
            let mut p = find(BodyType::Mime).cloned().unwrap_or(BodyPreference {
                body_type: BodyType::Mime,
                truncation_size: None,
                all_or_none: false,
                preview: None,
            });
            if p.truncation_size.is_none() {
                p.truncation_size = self.mime_truncation;
            }
            return p;
        }
        find(BodyType::Html)
            .or_else(|| find(BodyType::Plain))
            .cloned()
            .unwrap_or(BodyPreference {
                body_type: BodyType::Plain,
                truncation_size: None,
                all_or_none: false,
                preview: None,
            })
    }

    /// Extra `Email/get` arguments for the chosen body type.
    pub fn get_args(&self) -> Value {
        let pref = self.chosen();
        let mut args = json!({
            "properties": PROPERTIES,
            "bodyProperties": ["partId", "blobId", "size", "type", "name", "disposition", "cid"],
        });
        match pref.body_type {
            BodyType::Plain => args["fetchTextBodyValues"] = json!(true),
            BodyType::Html => args["fetchHTMLBodyValues"] = json!(true),
            BodyType::Mime => {}
        }
        args
    }

    /// Whether the full message has to be downloaded for each item.
    pub fn needs_mime(&self) -> bool {
        self.chosen().body_type == BodyType::Mime
    }
}

/// `MIMETruncation` codes to sizes (MS-ASCMD 2.2.3.108).
pub fn mime_truncation_size(code: u8) -> Option<usize> {
    match code {
        0 => Some(0),
        1 => Some(4_096),
        2 => Some(5_120),
        3 => Some(7_168),
        4 => Some(10_240),
        5 => Some(20_480),
        6 => Some(51_200),
        7 => Some(102_400),
        _ => None,
    }
}

fn str_of<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or_default()
}

fn keyword(email: &Value, k: &str) -> bool {
    email
        .get("keywords")
        .and_then(|m| m.get(k))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// User keywords, which EAS shows as categories.
fn categories(email: &Value) -> Vec<String> {
    let mut out: Vec<String> = email
        .get("keywords")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .filter(|(k, v)| !k.starts_with('$') && v.as_bool() == Some(true))
                .map(|(k, _)| k.clone())
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

/// Fingerprint of what the device shows that can change without the message
/// itself changing: read and flag state and categories.
pub fn state_hash(email: &Value) -> String {
    format!(
        "r{}f{}c{}",
        keyword(email, "$seen") as u8,
        keyword(email, "$flagged") as u8,
        categories(email).join("\u{1f}")
    )
}

fn addresses(email: &Value, key: &str) -> Vec<(String, String)> {
    email
        .get(key)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|x| {
                    (
                        str_of(x, "name").to_string(),
                        str_of(x, "email").to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

fn format_addresses(list: &[(String, String)]) -> String {
    list.iter()
        .map(|(name, addr)| {
            if name.is_empty() {
                addr.clone()
            } else {
                format!("\"{}\" <{addr}>", name.replace('"', "'"))
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// EAS dates: `2026-10-19T13:05:53.000Z`.
pub fn eas_date(jmap_date: &str) -> Option<String> {
    let dt = DateTime::parse_from_rfc3339(jmap_date).ok()?;
    Some(
        dt.with_timezone(&Utc)
            .format("%Y-%m-%dT%H:%M:%S%.3fZ")
            .to_string(),
    )
}

fn thread_topic(subject: &str) -> &str {
    let mut s = subject.trim();
    loop {
        let lower = s.to_ascii_lowercase();
        let Some(p) = ["re:", "fw:", "fwd:", "aw:", "wg:"]
            .iter()
            .find(|p| lower.starts_with(*p))
        else {
            return s;
        };
        s = s[p.len()..].trim_start();
    }
}

/// Concatenated values of the parts listed under `list` (`textBody` or
/// `htmlBody`), with whether any of them is HTML.
fn body_text(email: &Value, list: &str) -> (String, bool, usize) {
    let mut out = String::new();
    let mut html = false;
    let mut size = 0;
    let values = email.get("bodyValues");
    for part in email
        .get(list)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        size += part.get("size").and_then(Value::as_u64).unwrap_or(0) as usize;
        if str_of(part, "type").eq_ignore_ascii_case("text/html") {
            html = true;
        }
        if let Some(v) = values
            .and_then(|v| v.get(str_of(part, "partId")))
            .and_then(|v| v.get("value"))
            .and_then(Value::as_str)
        {
            out.push_str(v);
        }
    }
    (out, html, size)
}

fn native_body_type(email: &Value) -> u8 {
    let html = email
        .get("htmlBody")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .any(|p| str_of(p, "type").eq_ignore_ascii_case("text/html"));
    if html {
        2
    } else {
        1
    }
}

fn body_element(email: &Value, pref: &BodyPreference, mime: Option<&[u8]>) -> Element {
    let full = match pref.body_type {
        BodyType::Mime => String::from_utf8_lossy(mime.unwrap_or_default()).into_owned(),
        BodyType::Html => {
            let (text, is_html, _) = body_text(email, "htmlBody");
            if is_html {
                text
            } else {
                text_to_html(&text)
            }
        }
        BodyType::Plain => {
            let (text, is_html, _) = body_text(email, "textBody");
            if is_html {
                html_to_text(&text)
            } else {
                text
            }
        }
    };
    let mut body = Element::new(Page::AirSyncBase, "Body")
        .with_text_child("Type", (pref.body_type as u8).to_string())
        .with_text_child("EstimatedDataSize", full.len().to_string());
    match pref.truncation_size {
        Some(max) if full.len() > max => {
            body = body.with_text_child("Truncated", "1");
            if !pref.all_or_none && max > 0 {
                body = body.with_text_child("Data", truncate_utf8(&full, max));
            }
        }
        _ => {
            if !full.is_empty() {
                body = body.with_text_child("Data", full);
            }
        }
    }
    if let Some(n) = pref.preview.filter(|n| *n > 0) {
        let preview = str_of(email, "preview");
        if !preview.is_empty() {
            body = body.with_text_child("Preview", truncate_utf8(preview, n));
        }
    }
    body
}

fn attachments_element(email: &Value) -> Option<Element> {
    let id = str_of(email, "id");
    let list = email.get("attachments").and_then(Value::as_array)?;
    if list.is_empty() {
        return None;
    }
    let mut out = Element::new(Page::AirSyncBase, "Attachments");
    for a in list {
        let blob = str_of(a, "blobId");
        if blob.is_empty() {
            continue;
        }
        let name = a
            .get("name")
            .and_then(Value::as_str)
            .filter(|n| !n.is_empty())
            .unwrap_or("attachment");
        let mut el = Element::new(Page::AirSyncBase, "Attachment")
            .with_text_child("DisplayName", name)
            .with_text_child("FileReference", file_reference(id, blob))
            .with_text_child("Method", "1")
            .with_text_child(
                "EstimatedDataSize",
                a.get("size")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
                    .to_string(),
            );
        let cid = str_of(a, "cid").trim_matches(['<', '>']);
        if !cid.is_empty() {
            el = el.with_text_child("ContentId", cid);
        }
        if str_of(a, "disposition").eq_ignore_ascii_case("inline") && !cid.is_empty() {
            el = el.with_text_child("IsInline", "1");
        }
        out.push(el);
    }
    Some(out)
}

/// Attachment reference handed to the device: the email and blob IDs, which
/// `ItemOperations` and `GetAttachment` resolve again.
pub fn file_reference(email_id: &str, blob_id: &str) -> String {
    format!("{email_id}:{blob_id}")
}

/// Splits a [`file_reference`] into email and blob ID.
pub fn parse_file_reference(r: &str) -> Option<(&str, &str)> {
    r.split_once(':')
        .filter(|(e, b)| !e.is_empty() && !b.is_empty())
}

/// The `ApplicationData` of an email item. `mime` must hold the full
/// message when the chosen body type is MIME.
pub fn application_data(email: &Value, options: &BodyOptions, mime: Option<&[u8]>) -> Element {
    let pref = options.chosen();
    let mut d = Element::new(Page::AirSync, "ApplicationData");
    let e = |name: &str, text: String| Element::new(Page::Email, name).with_text(text);

    let to = addresses(email, "to");
    if !to.is_empty() {
        d.push(e("To", format_addresses(&to)));
    }
    let cc = addresses(email, "cc");
    if !cc.is_empty() {
        d.push(e("Cc", format_addresses(&cc)));
    }
    let from = addresses(email, "from");
    if !from.is_empty() {
        d.push(e("From", format_addresses(&from)));
    }
    let subject = str_of(email, "subject");
    d.push(e("Subject", subject.to_string()));
    let reply_to = addresses(email, "replyTo");
    if !reply_to.is_empty() {
        d.push(e("ReplyTo", format_addresses(&reply_to)));
    }
    if let Some(date) = eas_date(str_of(email, "receivedAt")) {
        d.push(e("DateReceived", date));
    }
    let display_to = to
        .iter()
        .map(|(n, a)| if n.is_empty() { a.clone() } else { n.clone() })
        .collect::<Vec<_>>()
        .join("; ");
    d.push(e("DisplayTo", display_to));
    d.push(e("ThreadTopic", thread_topic(subject).to_string()));
    d.push(e("Importance", "1".into()));
    d.push(e("Read", (keyword(email, "$seen") as u8).to_string()));
    if pref.body_type != BodyType::Mime {
        if let Some(a) = attachments_element(email) {
            d.push(a);
        }
    }
    d.push(body_element(email, &pref, mime));
    d.push(e("MessageClass", "IPM.Note".into()));
    d.push(e("InternetCPID", "65001".into()));
    let mut flag = Element::new(Page::Email, "Flag");
    if keyword(email, "$flagged") {
        flag = flag
            .with_text_child("FlagStatus", "2")
            .with_text_child("FlagType", "Flag for follow up");
    }
    d.push(flag);
    d.push(e("ContentClass", "urn:content-classes:message".into()));
    d.push(
        Element::new(Page::AirSyncBase, "NativeBodyType")
            .with_text(native_body_type(email).to_string()),
    );
    let thread = str_of(email, "threadId");
    if !thread.is_empty() {
        d.push(Element::new(Page::Email2, "ConversationId").with_opaque(thread.as_bytes()));
    }
    let cats = categories(email);
    if !cats.is_empty() {
        let mut c = Element::new(Page::Email, "Categories");
        for cat in cats {
            c.push(e("Category", cat));
        }
        d.push(c);
    }
    d
}

/// `ApplicationData` for a server-side `Change`: only the state that
/// [`state_hash`] covers, so the device keeps the rest of the item.
pub fn change_data(email: &Value) -> Element {
    let mut d = Element::new(Page::AirSync, "ApplicationData").with_child(
        Element::new(Page::Email, "Read").with_text((keyword(email, "$seen") as u8).to_string()),
    );
    let mut flag = Element::new(Page::Email, "Flag");
    if keyword(email, "$flagged") {
        flag = flag
            .with_text_child("FlagStatus", "2")
            .with_text_child("FlagType", "Flag for follow up");
    }
    d.push(flag);
    let mut c = Element::new(Page::Email, "Categories");
    for cat in categories(email) {
        c.push(Element::new(Page::Email, "Category").with_text(cat));
    }
    d.push(c);
    d
}

/// JMAP patch for a device-side `Change` of an email. Only what the device
/// sent is patched, so properties it doesn't know are left alone.
pub fn change_patch(data: &Element) -> Map<String, Value> {
    let mut patch = Map::new();
    if let Some(read) = data.child_text(Page::Email, "Read") {
        let v = if read.trim() == "1" {
            json!(true)
        } else {
            Value::Null
        };
        patch.insert("keywords/$seen".into(), v);
    }
    if let Some(flag) = data.child(Page::Email, "Flag") {
        let flagged = flag
            .child_text(Page::Email, "FlagStatus")
            .is_some_and(|s| s.trim() == "2");
        patch.insert(
            "keywords/$flagged".into(),
            if flagged { json!(true) } else { Value::Null },
        );
    }
    patch
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Value {
        json!({
            "id": "m1",
            "blobId": "b1",
            "threadId": "t1",
            "keywords": {"$seen": true, "work": true},
            "receivedAt": "2026-10-19T13:05:53Z",
            "from": [{"name": "Alice", "email": "alice@example.org"}],
            "to": [{"name": null, "email": "bob@example.org"}],
            "subject": "Re: Fwd: Plans",
            "preview": "Hello there",
            "textBody": [{"partId": "1", "type": "text/plain", "size": 11}],
            "htmlBody": [{"partId": "2", "type": "text/html", "size": 30}],
            "attachments": [{"blobId": "b2", "name": "a.pdf", "size": 1234, "type": "application/pdf", "disposition": "attachment"}],
            "bodyValues": {
                "1": {"value": "Hello there"},
                "2": {"value": "<p>Hello <b>there</b></p>"}
            }
        })
    }

    fn html_opts(trunc: Option<usize>) -> BodyOptions {
        BodyOptions {
            preferences: vec![
                BodyPreference {
                    body_type: BodyType::Plain,
                    truncation_size: None,
                    all_or_none: false,
                    preview: None,
                },
                BodyPreference {
                    body_type: BodyType::Html,
                    truncation_size: trunc,
                    all_or_none: false,
                    preview: Some(5),
                },
            ],
            mime_support: 0,
            mime_truncation: None,
        }
    }

    #[test]
    fn serialises_core_fields() {
        let d = application_data(&sample(), &html_opts(None), None);
        assert_eq!(
            d.child_text(Page::Email, "From").as_deref(),
            Some("\"Alice\" <alice@example.org>")
        );
        assert_eq!(
            d.child_text(Page::Email, "To").as_deref(),
            Some("bob@example.org")
        );
        assert_eq!(
            d.child_text(Page::Email, "DateReceived").as_deref(),
            Some("2026-10-19T13:05:53.000Z")
        );
        assert_eq!(
            d.child_text(Page::Email, "ThreadTopic").as_deref(),
            Some("Plans")
        );
        assert_eq!(d.child_text(Page::Email, "Read").as_deref(), Some("1"));
        let body = d.child(Page::AirSyncBase, "Body").unwrap();
        assert_eq!(
            body.child_text(Page::AirSyncBase, "Type").as_deref(),
            Some("2")
        );
        assert_eq!(
            body.child_text(Page::AirSyncBase, "Data").as_deref(),
            Some("<p>Hello <b>there</b></p>")
        );
        assert_eq!(
            body.child_text(Page::AirSyncBase, "Preview").as_deref(),
            Some("Hello")
        );
        let att = d.child(Page::AirSyncBase, "Attachments").unwrap();
        assert_eq!(
            att.child(Page::AirSyncBase, "Attachment")
                .unwrap()
                .child_text(Page::AirSyncBase, "FileReference")
                .as_deref(),
            Some("m1:b2")
        );
        let cats = d.child(Page::Email, "Categories").unwrap();
        assert_eq!(
            cats.child_text(Page::Email, "Category").as_deref(),
            Some("work")
        );
        // Must encode: every tag exists on its page.
        let bytes = eas_wbxml::encode(&d);
        assert_eq!(eas_wbxml::decode(&bytes).unwrap(), d);
    }

    #[test]
    fn truncates_and_falls_back_to_text() {
        let d = application_data(&sample(), &html_opts(Some(8)), None);
        let body = d.child(Page::AirSyncBase, "Body").unwrap();
        assert_eq!(
            body.child_text(Page::AirSyncBase, "Truncated").as_deref(),
            Some("1")
        );
        assert_eq!(
            body.child_text(Page::AirSyncBase, "Data").as_deref(),
            Some("<p>Hello")
        );

        let plain = BodyOptions {
            preferences: vec![BodyPreference {
                body_type: BodyType::Plain,
                truncation_size: None,
                all_or_none: false,
                preview: None,
            }],
            ..Default::default()
        };
        let mut html_only = sample();
        html_only["textBody"] = json!([{"partId": "2", "type": "text/html"}]);
        let d = application_data(&html_only, &plain, None);
        let body = d.child(Page::AirSyncBase, "Body").unwrap();
        assert_eq!(
            body.child_text(Page::AirSyncBase, "Data").as_deref(),
            Some("Hello there")
        );
    }

    #[test]
    fn mime_choice_and_body() {
        let opts = BodyOptions {
            preferences: vec![BodyPreference {
                body_type: BodyType::Mime,
                truncation_size: None,
                all_or_none: false,
                preview: None,
            }],
            mime_support: 0,
            mime_truncation: None,
        };
        assert!(opts.needs_mime());
        assert!(!html_opts(None).needs_mime());
        let d = application_data(&sample(), &opts, Some(b"Subject: x\r\n\r\nhi"));
        assert!(d.child(Page::AirSyncBase, "Attachments").is_none());
        let body = d.child(Page::AirSyncBase, "Body").unwrap();
        assert_eq!(
            body.child_text(Page::AirSyncBase, "Type").as_deref(),
            Some("4")
        );
        assert_eq!(
            body.child_text(Page::AirSyncBase, "Data").as_deref(),
            Some("Subject: x\r\n\r\nhi")
        );
    }

    #[test]
    fn hash_and_patch() {
        let mut e = sample();
        let h1 = state_hash(&e);
        e["keywords"]["$flagged"] = json!(true);
        assert_ne!(h1, state_hash(&e));

        let change = Element::new(Page::AirSync, "ApplicationData")
            .with_child(Element::new(Page::Email, "Read").with_text("0"))
            .with_child(
                Element::new(Page::Email, "Flag")
                    .with_child(Element::new(Page::Email, "FlagStatus").with_text("2")),
            );
        let p = change_patch(&change);
        assert_eq!(p["keywords/$seen"], Value::Null);
        assert_eq!(p["keywords/$flagged"], json!(true));

        let only_read = Element::new(Page::AirSync, "ApplicationData")
            .with_child(Element::new(Page::Email, "Read").with_text("1"));
        let p = change_patch(&only_read);
        assert_eq!(p.len(), 1);
    }

    #[test]
    fn file_references() {
        assert_eq!(parse_file_reference("m1:b2"), Some(("m1", "b2")));
        assert_eq!(parse_file_reference("m1"), None);
        assert_eq!(parse_file_reference(":b"), None);
    }
}
