//! `SendMail`, `SmartReply` and `SmartForward` (MS-ASCMD 2.2.1.17-19).
//!
//! The device's MIME message is stored in Sent (or a scratch copy that is
//! destroyed after submission) and submitted with an envelope built from
//! its headers, so `Bcc` recipients get the mail without appearing in it.
//! Success is an empty 200; failures answer with a ComposeMail status.

use super::{empty, wbxml, Ctx, HandlerError, HandlerResult};
use eas_convert::compose::{self, Original, PrepareError, ORIGINAL_PROPERTIES};
use eas_jmap::mail::pick_identity;
use eas_jmap::JmapError;
use eas_proto::Command;
use eas_wbxml::{Element, Page};
use serde_json::{json, Map, Value};

/// ComposeMail status codes (MS-ASCMD 2.2.2).
mod status {
    pub const INVALID_MIME: u8 = 107;
    pub const SERVER_ERROR: u8 = 110;
    pub const RETRY_LATER: u8 = 111;
    pub const QUOTA_EXCEEDED: u8 = 113;
    pub const PREVIOUSLY_SENT: u8 = 118;
    pub const NO_RECIPIENT: u8 = 119;
    pub const SUBMISSION_FAILED: u8 = 120;
    pub const ITEM_NOT_FOUND: u8 = 150;
}

/// Largest total size of original attachments carried into a forward.
const MAX_FORWARDED: u64 = 50 * 1024 * 1024;

fn fail(command: Command, code: u8) -> HandlerResult {
    let (page, root) = command
        .status_root()
        .unwrap_or((Page::ComposeMail, "SendMail"));
    Ok(wbxml(
        &Element::new(page, root).with_text_child("Status", code.to_string()),
    ))
}

/// The JMAP ID of the item a smart command refers to.
fn source_id(req: &Element) -> Option<String> {
    let src = req.child(Page::ComposeMail, "Source")?;
    if let Some(item) = src.child_text(Page::ComposeMail, "ItemId") {
        return Some(match item.split_once(':') {
            Some((_, id)) => id.to_string(),
            None => item,
        });
    }
    // Search results hand out the JMAP ID as the LongId.
    src.child_text(Page::ComposeMail, "LongId")
}

async fn load_original(
    ctx: &Ctx<'_>,
    id: &str,
    forward: bool,
) -> Result<Option<Original>, HandlerError> {
    let args = json!({
        "properties": ORIGINAL_PROPERTIES,
        "bodyProperties": ["partId", "blobId", "size", "type", "name", "cid", "disposition"],
        "fetchTextBodyValues": true,
        "fetchHTMLBodyValues": true,
    });
    let emails = ctx.jmap.email_get(&[id.to_string()], &args).await?;
    let Some(email) = emails.first() else {
        return Ok(None);
    };
    let mut original = Original::from_jmap(email);
    let sizes: Vec<u64> = email
        .get("attachments")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|x| x.get("size").and_then(Value::as_u64).unwrap_or(0))
                .collect()
        })
        .unwrap_or_default();
    let mut total = 0;
    for (a, size) in original.attachments.iter_mut().zip(sizes) {
        // A reply only carries inline images its quoted HTML refers to.
        let wanted = forward || a.cid.is_some();
        if !wanted || total + size > MAX_FORWARDED {
            continue;
        }
        total += size;
        a.data = ctx.jmap.download(&a.blob_id, &a.name).await?;
    }
    Ok(Some(original))
}

pub async fn handle(ctx: &Ctx<'_>, command: Command, request: Option<Element>) -> HandlerResult {
    let req = request.ok_or(HandlerError::BadRequest("empty compose request"))?;
    let Some(mime) = req
        .child(Page::ComposeMail, "Mime")
        .and_then(Element::bytes)
    else {
        return fail(command, status::INVALID_MIME);
    };
    let save = req.child(Page::ComposeMail, "SaveInSentItems").is_some();
    let client_id = req.child_text(Page::ComposeMail, "ClientId");
    let store = &ctx.state.store;
    let dev = ctx.device.id;
    let client_key = client_id.as_ref().map(|c| format!("send:{c}"));
    if let Some(key) = &client_key {
        if store.client_id(dev, key).await?.is_some() {
            tracing::info!(device = %ctx.line.device_id, %command, "duplicate ClientId, not sending again");
            return fail(command, status::PREVIOUSLY_SENT);
        }
    }

    let smart = matches!(command, Command::SmartReply | Command::SmartForward);
    let forward = command == Command::SmartForward;
    let replace = req.child(Page::ComposeMail, "ReplaceMime").is_some();
    let mut original_id = None;
    let full = if smart {
        let Some(id) = source_id(&req) else {
            return fail(command, status::ITEM_NOT_FOUND);
        };
        original_id = Some(id.clone());
        if replace {
            mime
        } else {
            let Some(original) = load_original(ctx, &id, forward).await? else {
                return fail(command, status::ITEM_NOT_FOUND);
            };
            match compose::smart(&mime, &original, forward) {
                Some(m) => m,
                None => return fail(command, status::INVALID_MIME),
            }
        }
    } else {
        mime
    };

    let out = match compose::prepare(&full) {
        Ok(o) => o,
        Err(PrepareError::NoRecipient) => return fail(command, status::NO_RECIPIENT),
        Err(_) => return fail(command, status::INVALID_MIME),
    };

    let (mailboxes, _) = ctx.jmap.mailboxes().await?;
    let role = |r: &str| {
        mailboxes
            .iter()
            .find(|m| m.role.as_deref() == Some(r))
            .map(|m| m.id.clone())
    };
    let Some(mailbox) = role("sent").or_else(|| role("drafts")) else {
        tracing::warn!(user = %ctx.device.username, "no Sent or Drafts mailbox to submit from");
        return fail(command, status::SERVER_ERROR);
    };
    let identities = ctx.jmap.identities().await?;
    let Some(identity) = pick_identity(&identities, &out.envelope.mail_from) else {
        return fail(command, status::SUBMISSION_FAILED);
    };
    let sent = ctx
        .jmap
        .submit(
            out.mime,
            &identity.id,
            &out.envelope.mail_from,
            &out.envelope.rcpt_to,
            &mailbox,
            save && role("sent").is_some(),
        )
        .await;
    let email_id = match sent {
        Ok(id) => id,
        Err(JmapError::Method {
            method,
            kind,
            description,
        }) => {
            tracing::warn!(user = %ctx.device.username, %method, %kind, ?description, "sending failed");
            let code = match kind.as_str() {
                "overQuota" => status::QUOTA_EXCEEDED,
                "invalidEmail" => status::INVALID_MIME,
                _ => status::SUBMISSION_FAILED,
            };
            return fail(command, code);
        }
        Err(e @ (JmapError::Unauthorized | JmapError::Throttled)) => return Err(e.into()),
        Err(e) => {
            tracing::warn!(error = %e, "sending failed");
            return fail(command, status::RETRY_LATER);
        }
    };
    if let Some(key) = &client_key {
        store.record_client_id(dev, key, &email_id).await?;
    }
    tracing::info!(user = %ctx.device.username, device = %ctx.line.device_id, %command, recipients = out.envelope.rcpt_to.len(), "mail sent");

    // Mark the original answered or forwarded, as desktop clients do.
    if let Some(id) = original_id {
        let kw = if forward { "$forwarded" } else { "$answered" };
        let mut update = Map::new();
        update.insert(id, json!({ format!("keywords/{kw}"): true }));
        if let Err(e) = ctx.jmap.email_set(update, Vec::new()).await {
            tracing::debug!(error = %e, "could not flag the original");
        }
    }
    Ok(empty())
}

/// Turns a pre-14.0 style request (raw MIME body, options in the request
/// line) into the WBXML form the handler reads.
pub fn legacy_request(
    line: &eas_proto::RequestLine,
    command: Command,
    mime: &[u8],
) -> Option<Element> {
    let (page, root) = command.status_root()?;
    let mut el = Element::new(page, root);
    if line.save_in_sent {
        el.push(Element::new(Page::ComposeMail, "SaveInSentItems"));
    }
    if let Some(item) = &line.item_id {
        let mut src =
            Element::new(Page::ComposeMail, "Source").with_text_child("ItemId", item.clone());
        if let Some(f) = &line.collection_id {
            src = src.with_text_child("FolderId", f.clone());
        }
        el.push(src);
    }
    el.push(Element::new(Page::ComposeMail, "Mime").with_opaque(mime.to_vec()));
    Some(el)
}
