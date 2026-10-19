//! Outgoing mail: the MIME message a device sends with `SendMail`,
//! `SmartReply` or `SmartForward`.
//!
//! [`prepare`] derives the SMTP envelope and removes `Bcc` from the copy
//! that is stored and delivered. [`smart`] builds the full message for the
//! smart commands, where the device sends only its new text and the server
//! adds the original: quoted for a reply, with its attachments for a
//! forward.

use crate::text::text_to_html;
use mail_builder::headers::raw::Raw;
use mail_builder::MessageBuilder;
use mail_parser::{HeaderName, MessageParser, MimeHeaders};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Envelope {
    pub mail_from: String,
    pub rcpt_to: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Outgoing {
    /// The message without `Bcc`.
    pub mime: Vec<u8>,
    pub envelope: Envelope,
}

fn addresses(a: Option<&mail_parser::Address<'_>>) -> Vec<String> {
    a.map(|a| {
        a.iter()
            .filter_map(|x| x.address())
            .map(str::to_string)
            .collect()
    })
    .unwrap_or_default()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrepareError {
    Unparsable,
    NoSender,
    NoRecipient,
}

/// Envelope and `Bcc`-free copy of a device-built message.
pub fn prepare(mime: &[u8]) -> Result<Outgoing, PrepareError> {
    let msg = MessageParser::default()
        .parse(mime)
        .ok_or(PrepareError::Unparsable)?;
    let mail_from = addresses(msg.from())
        .into_iter()
        .next()
        .ok_or(PrepareError::NoSender)?;
    let mut rcpt_to: Vec<String> = Vec::new();
    for a in addresses(msg.to())
        .into_iter()
        .chain(addresses(msg.cc()))
        .chain(addresses(msg.bcc()))
    {
        if !rcpt_to.iter().any(|r| r.eq_ignore_ascii_case(&a)) {
            rcpt_to.push(a);
        }
    }
    if rcpt_to.is_empty() {
        return Err(PrepareError::NoRecipient);
    }
    // Cut every top-level Bcc field, name through the end of its value.
    let mut out = Vec::with_capacity(mime.len());
    let mut pos = 0usize;
    for h in msg.headers() {
        if h.name == HeaderName::Bcc {
            let start = h.offset_field as usize;
            let mut end = h.offset_end as usize;
            if start < pos || end > mime.len() {
                continue;
            }
            // offset_end points past the value; take the line break too.
            while end < mime.len() && matches!(mime[end], b'\r' | b'\n') {
                end += 1;
                if mime[end - 1] == b'\n' {
                    break;
                }
            }
            out.extend_from_slice(&mime[pos..start]);
            pos = end;
        }
    }
    out.extend_from_slice(&mime[pos..]);
    Ok(Outgoing {
        mime: out,
        envelope: Envelope { mail_from, rcpt_to },
    })
}

/// An attachment of the original message.
#[derive(Debug, Clone)]
pub struct Attachment {
    pub name: String,
    pub content_type: String,
    pub cid: Option<String>,
    pub blob_id: String,
    pub data: Vec<u8>,
}

/// The message being replied to or forwarded.
#[derive(Debug, Clone, Default)]
pub struct Original {
    pub from: String,
    pub to: String,
    pub date: String,
    pub subject: String,
    pub message_id: Option<String>,
    pub references: Vec<String>,
    pub text: String,
    pub html: Option<String>,
    /// Filled by the caller, which downloads the blobs it needs.
    pub attachments: Vec<Attachment>,
}

/// JMAP properties [`Original::from_jmap`] reads.
pub const ORIGINAL_PROPERTIES: &[&str] = &[
    "id",
    "from",
    "to",
    "sentAt",
    "receivedAt",
    "subject",
    "messageId",
    "references",
    "textBody",
    "htmlBody",
    "attachments",
    "bodyValues",
];

fn jmap_addresses(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|x| {
                    let email = x.get("email").and_then(Value::as_str).unwrap_or_default();
                    match x.get("name").and_then(Value::as_str) {
                        Some(n) if !n.is_empty() => format!("{n} <{email}>"),
                        _ => email.to_string(),
                    }
                })
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default()
}

fn body_values(v: &Value, list: &str) -> (String, bool) {
    let mut out = String::new();
    let mut html = false;
    for part in v.get(list).and_then(Value::as_array).into_iter().flatten() {
        let id = part
            .get("partId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if part
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|t| t.eq_ignore_ascii_case("text/html"))
        {
            html = true;
        }
        if let Some(s) = v
            .get("bodyValues")
            .and_then(|b| b.get(id))
            .and_then(|b| b.get("value"))
            .and_then(Value::as_str)
        {
            out.push_str(s);
        }
    }
    (out, html)
}

impl Original {
    /// From a JMAP `Email` fetched with [`ORIGINAL_PROPERTIES`] and both
    /// text and HTML body values. Attachments are listed without data.
    pub fn from_jmap(v: &Value) -> Original {
        let s = |k: &str| {
            v.get(k)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        let ids = |k: &str| -> Vec<String> {
            v.get(k)
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default()
        };
        let (text, text_is_html) = body_values(v, "textBody");
        let (html, html_is_html) = body_values(v, "htmlBody");
        let text = if text_is_html {
            crate::text::html_to_text(&text)
        } else {
            text
        };
        let date = Some(s("sentAt"))
            .filter(|d| !d.is_empty())
            .unwrap_or_else(|| s("receivedAt"));
        let attachments = v
            .get("attachments")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|a| {
                let blob_id = a.get("blobId")?.as_str()?.to_string();
                let get = |k: &str| a.get(k).and_then(Value::as_str).map(str::to_string);
                Some(Attachment {
                    name: get("name").unwrap_or_else(|| "attachment".into()),
                    content_type: get("type").unwrap_or_else(|| "application/octet-stream".into()),
                    cid: get("cid").map(|c| c.trim_matches(['<', '>']).to_string()),
                    blob_id,
                    data: Vec::new(),
                })
            })
            .collect();
        Original {
            from: jmap_addresses(v, "from"),
            to: jmap_addresses(v, "to"),
            date,
            subject: s("subject"),
            message_id: ids("messageId").into_iter().next(),
            references: ids("references"),
            text,
            html: html_is_html.then_some(html),
            attachments,
        }
    }

    fn header_text(&self) -> String {
        format!(
            "-------- Original Message --------\r\nFrom: {}\r\nDate: {}\r\nSubject: {}\r\nTo: {}\r\n\r\n",
            self.from, self.date, self.subject, self.to
        )
    }
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn quote(text: &str) -> String {
    text.lines()
        .map(|l| format!("> {l}"))
        .collect::<Vec<_>>()
        .join("\r\n")
}

/// Headers the rebuilt message must not copy; mail-builder writes its own.
fn skip_header(lower: &str) -> bool {
    lower.starts_with("content-") || lower == "mime-version"
}

fn canonical_name(name: &str) -> String {
    match name.to_ascii_lowercase().as_str() {
        "message-id" => "Message-ID".into(),
        "date" => "Date".into(),
        "in-reply-to" => "In-Reply-To".into(),
        "references" => "References".into(),
        _ => name.to_string(),
    }
}

/// The full message for `SmartReply` (`forward == false`) or
/// `SmartForward`: the device's headers and new text, followed by the
/// original. A reply also keeps the original's inline images, a forward
/// all its attachments.
pub fn smart(device_mime: &[u8], original: &Original, forward: bool) -> Option<Vec<u8>> {
    let msg = MessageParser::default().parse(device_mime)?;
    let mut b = MessageBuilder::new();
    let mut has_in_reply_to = false;
    let mut has_references = false;
    for h in msg.headers() {
        let name = h.name.as_str();
        let lower = name.to_ascii_lowercase();
        if skip_header(&lower) {
            continue;
        }
        has_in_reply_to |= lower == "in-reply-to";
        has_references |= lower == "references";
        let (start, end) = (h.offset_start as usize, h.offset_end as usize);
        let Some(raw) = device_mime.get(start..end) else {
            continue;
        };
        let value = String::from_utf8_lossy(raw)
            .replace("\r\n", "")
            .replace('\n', "")
            .trim()
            .to_string();
        b = b.header(canonical_name(name), Raw::new(value));
    }
    if !forward {
        if let Some(id) = &original.message_id {
            if !has_in_reply_to {
                b = b.header("In-Reply-To", Raw::new(format!("<{id}>")));
            }
            if !has_references {
                let mut refs: Vec<String> = original
                    .references
                    .iter()
                    .map(|r| format!("<{r}>"))
                    .collect();
                refs.push(format!("<{id}>"));
                b = b.header("References", Raw::new(refs.join(" ")));
            }
        }
    }

    let new_text = msg.body_text(0).map(|t| t.into_owned()).unwrap_or_default();
    let new_html = msg.body_html(0).map(|t| t.into_owned());
    // mail-parser converts text to HTML when a message has only text; only
    // treat HTML as the device's own if it sent an HTML part.
    let device_sent_html = msg.html_body.iter().any(|i| {
        msg.parts
            .get(*i as usize)
            .is_some_and(|p| p.is_content_type("text", "html"))
    });

    let header = original.header_text();
    let quoted_text = if forward {
        original.text.clone()
    } else {
        quote(&original.text)
    };
    let text = format!("{new_text}\r\n\r\n{header}{quoted_text}");
    b = b.text_body(text);

    if device_sent_html || original.html.is_some() {
        let mine = match (&new_html, device_sent_html) {
            (Some(h), true) => h.clone(),
            _ => text_to_html(&new_text),
        };
        let theirs = original
            .html
            .clone()
            .unwrap_or_else(|| text_to_html(&original.text));
        let header_html = escape(&header).replace("\r\n", "<br>\r\n");
        let html = format!(
            "{mine}\r\n<br>\r\n<div>{header_html}</div>\r\n<blockquote style=\"margin:0 0 0 .8ex;border-left:1px solid #ccc;padding-left:1ex\">{theirs}</blockquote>"
        );
        b = b.html_body(html);
    }

    for part in msg.attachments() {
        let ctype = part
            .content_type()
            .map(|c| match c.subtype() {
                Some(sub) => format!("{}/{}", c.ctype(), sub),
                None => c.ctype().to_string(),
            })
            .unwrap_or_else(|| "application/octet-stream".into());
        let data = part.contents().to_vec();
        let inline = part.content_disposition().is_some_and(|d| d.is_inline());
        match (part.content_id(), inline) {
            (Some(cid), true) => {
                b = b.inline(ctype, cid.trim_matches(['<', '>']).to_string(), data);
            }
            _ => {
                let name = part.attachment_name().unwrap_or("attachment").to_string();
                b = b.attachment(ctype, name, data);
            }
        }
    }
    for a in &original.attachments {
        if a.data.is_empty() && !forward {
            continue;
        }
        match &a.cid {
            Some(cid) if original.html.is_some() => {
                b = b.inline(a.content_type.clone(), cid.clone(), a.data.clone());
            }
            _ if forward => {
                b = b.attachment(a.content_type.clone(), a.name.clone(), a.data.clone());
            }
            _ => {}
        }
    }
    b.write_to_vec().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const DEVICE: &[u8] = b"From: Bob <bob@example.org>\r\nTo: Alice <alice@example.org>\r\nCc: carol@example.org\r\nBcc: dave@example.org,\r\n eve@example.org\r\nSubject: Re: Plans\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nSounds good.\r\n";

    #[test]
    fn prepare_strips_bcc_and_builds_envelope() {
        let out = prepare(DEVICE).unwrap();
        assert_eq!(out.envelope.mail_from, "bob@example.org");
        assert_eq!(
            out.envelope.rcpt_to,
            vec![
                "alice@example.org",
                "carol@example.org",
                "dave@example.org",
                "eve@example.org"
            ]
        );
        let text = String::from_utf8(out.mime).unwrap();
        assert!(!text.to_ascii_lowercase().contains("bcc"), "{text}");
        assert!(!text.contains("eve@"), "{text}");
        assert!(text.contains("Subject: Re: Plans\r\n"));
        assert!(text.ends_with("Sounds good.\r\n"));
        assert_eq!(
            prepare(b"Subject: x\r\n\r\nno sender").unwrap_err(),
            PrepareError::NoSender
        );
        assert_eq!(
            prepare(b"From: a@b\r\nSubject: x\r\n\r\nnobody").unwrap_err(),
            PrepareError::NoRecipient
        );
    }

    fn original() -> Original {
        let mut o = Original::from_jmap(&json!({
            "from": [{"name": "Alice", "email": "alice@example.org"}],
            "to": [{"email": "bob@example.org"}],
            "sentAt": "2026-10-18T09:00:00Z",
            "subject": "Plans",
            "messageId": ["orig@example.org"],
            "references": ["older@example.org"],
            "textBody": [{"partId": "1", "type": "text/plain"}],
            "htmlBody": [{"partId": "1", "type": "text/plain"}],
            "bodyValues": {"1": {"value": "Shall we meet?\nAt 10?"}},
            "attachments": [{"blobId": "b1", "name": "agenda.pdf", "type": "application/pdf"}]
        }));
        o.attachments[0].data = b"%PDF-1.4".to_vec();
        o
    }

    #[test]
    fn smart_reply_quotes_and_threads() {
        let out = smart(DEVICE, &original(), false).unwrap();
        let parsed = MessageParser::default().parse(&out).unwrap();
        let text = parsed.body_text(0).unwrap();
        assert!(text.starts_with("Sounds good."), "{text}");
        assert!(text.contains("> Shall we meet?\r\n> At 10?"), "{text}");
        assert_eq!(parsed.in_reply_to().as_text(), Some("orig@example.org"));
        assert_eq!(parsed.subject(), Some("Re: Plans"));
        // Bcc survives here; prepare() removes it before sending.
        assert!(parsed.bcc().is_some());
        assert_eq!(parsed.attachments().count(), 0);
        assert_eq!(
            String::from_utf8_lossy(&out).matches("Message-ID").count(),
            1
        );
    }

    #[test]
    fn smart_forward_keeps_attachments() {
        let out = smart(DEVICE, &original(), true).unwrap();
        let parsed = MessageParser::default().parse(&out).unwrap();
        let text = parsed.body_text(0).unwrap();
        assert!(text.contains("Shall we meet?"), "{text}");
        assert!(!text.contains("> Shall"), "{text}");
        let att: Vec<_> = parsed.attachments().collect();
        assert_eq!(att.len(), 1);
        assert_eq!(att[0].attachment_name(), Some("agenda.pdf"));
        assert_eq!(att[0].contents(), b"%PDF-1.4");
    }
}
