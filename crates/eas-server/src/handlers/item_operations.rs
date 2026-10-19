//! `ItemOperations` (MS-ASCMD 2.2.1.10): full items and attachments on
//! demand, and emptying a folder; plus the older `GetAttachment`.

use super::folder_sync::KIND_MAILBOX;
use super::sync::{parse_options, serialise};
use super::{wbxml, Ctx, HandlerError, HandlerResult};
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use base64::Engine as _;
use eas_convert::email::parse_file_reference;
use eas_wbxml::{Element, Node, Page};
use serde_json::{json, Value};

/// ItemOperations status codes (MS-ASCMD 2.2.3.177.7).
mod status {
    pub const SUCCESS: u8 = 1;
    pub const PROTOCOL_ERROR: u8 = 2;
    pub const SERVER_ERROR: u8 = 3;
    pub const NOT_FOUND: u8 = 6;
    pub const STORE_UNKNOWN: u8 = 9;
    pub const TOO_LARGE: u8 = 11;
    pub const ACTION_NOT_SUPPORTED: u8 = 156;
}

/// Largest attachment returned inline; base64 makes it a third bigger.
const MAX_ATTACHMENT: u64 = 50 * 1024 * 1024;

fn op(name: &str, code: u8) -> Element {
    Element::new(Page::ItemOperations, name).with_text_child("Status", code.to_string())
}

/// Content type, name and size of an attachment blob of an email.
async fn attachment_info(
    ctx: &Ctx<'_>,
    email_id: &str,
    blob_id: &str,
) -> Result<Option<(String, String, u64)>, HandlerError> {
    let args = json!({
        "properties": ["id", "attachments"],
        "bodyProperties": ["blobId", "type", "name", "size"],
    });
    let emails = ctx.jmap.email_get(&[email_id.to_string()], &args).await?;
    Ok(emails
        .first()
        .and_then(|e| e.get("attachments"))
        .and_then(Value::as_array)
        .and_then(|list| {
            list.iter()
                .find(|a| a.get("blobId").and_then(Value::as_str) == Some(blob_id))
        })
        .map(|a| {
            let s = |k: &str| {
                a.get(k)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            };
            let ctype = Some(s("type"))
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| "application/octet-stream".into());
            let name = Some(s("name"))
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| "attachment".into());
            (
                ctype,
                name,
                a.get("size").and_then(Value::as_u64).unwrap_or(0),
            )
        }))
}

async fn fetch_attachment(ctx: &Ctx<'_>, reference: &str) -> Result<Element, HandlerError> {
    let with_ref = |el: Element| {
        el.with_child(Element::new(Page::AirSyncBase, "FileReference").with_text(reference))
    };
    let Some((email_id, blob_id)) = parse_file_reference(reference) else {
        return Ok(with_ref(op("Fetch", status::NOT_FOUND)));
    };
    let Some((ctype, _, size)) = attachment_info(ctx, email_id, blob_id).await? else {
        return Ok(with_ref(op("Fetch", status::NOT_FOUND)));
    };
    if size > MAX_ATTACHMENT {
        return Ok(with_ref(op("Fetch", status::TOO_LARGE)));
    }
    let data = ctx.jmap.download(blob_id, "attachment").await?;
    let props = Element::new(Page::ItemOperations, "Properties")
        .with_child(Element::new(Page::AirSyncBase, "ContentType").with_text(ctype))
        .with_child(
            Element::new(Page::ItemOperations, "Data")
                .with_text(base64::engine::general_purpose::STANDARD.encode(&data)),
        );
    Ok(with_ref(op("Fetch", status::SUCCESS)).with_child(props))
}

async fn fetch_item(ctx: &Ctx<'_>, fetch: &Element) -> Result<Element, HandlerError> {
    let cid = fetch
        .child_text(Page::AirSync, "CollectionId")
        .unwrap_or_default();
    let sid = fetch
        .child_text(Page::AirSync, "ServerId")
        .unwrap_or_default();
    let tagged = |el: Element| {
        el.with_child(Element::new(Page::AirSync, "CollectionId").with_text(cid.clone()))
            .with_child(Element::new(Page::AirSync, "ServerId").with_text(sid.clone()))
    };
    let folder = ctx.state.store.folder(ctx.device.id, &cid).await?;
    let jmap_id = sid.split_once(':').map(|(f, id)| (f, id.to_string()));
    let (Some(folder), Some((prefix, jmap_id))) = (folder, jmap_id) else {
        return Ok(tagged(op("Fetch", status::NOT_FOUND)));
    };
    if folder.kind != KIND_MAILBOX || prefix != cid {
        return Ok(tagged(op("Fetch", status::NOT_FOUND)));
    }
    let (_, opts) = parse_options(fetch.child(Page::ItemOperations, "Options"));
    let mut found = serialise(ctx, &[jmap_id], &opts).await?;
    let Some((_, _, data)) = found.pop() else {
        return Ok(tagged(op("Fetch", status::NOT_FOUND)));
    };
    // Properties carries what ApplicationData would in a Sync.
    let mut props = Element::new(Page::ItemOperations, "Properties");
    props.children = data.children;
    Ok(tagged(op("Fetch", status::SUCCESS))
        .with_child(Element::new(Page::AirSync, "Class").with_text("Email"))
        .with_child(props))
}

async fn empty_folder(ctx: &Ctx<'_>, req: &Element) -> Result<Element, HandlerError> {
    let cid = req
        .child_text(Page::AirSync, "CollectionId")
        .unwrap_or_default();
    let tagged = |el: Element| {
        el.with_child(Element::new(Page::AirSync, "CollectionId").with_text(cid.clone()))
    };
    let folder = match ctx.state.store.folder(ctx.device.id, &cid).await? {
        Some(f) if f.kind == KIND_MAILBOX => f,
        _ => return Ok(tagged(op("EmptyFolderContents", status::NOT_FOUND))),
    };
    let ids = ctx.jmap.email_ids(&folder.jmap_id, None).await?;
    let mut failed = 0;
    for chunk in ids.chunks(500) {
        failed += ctx
            .jmap
            .email_set(Default::default(), chunk.to_vec())
            .await?
            .len();
    }
    tracing::info!(device = %ctx.line.device_id, folder = %cid, emptied = ids.len(), failed, "EmptyFolderContents");
    let code = if failed == 0 {
        status::SUCCESS
    } else {
        status::SERVER_ERROR
    };
    Ok(tagged(op("EmptyFolderContents", code)))
}

pub async fn handle(ctx: &Ctx<'_>, request: Option<Element>) -> HandlerResult {
    let req = request.ok_or(HandlerError::BadRequest("empty ItemOperations"))?;
    if !req.is(Page::ItemOperations, "ItemOperations") {
        return Ok(wbxml(&op("ItemOperations", status::PROTOCOL_ERROR)));
    }
    let mut response = Element::new(Page::ItemOperations, "Response");
    for child in req.elements() {
        let el = match child.name() {
            "Fetch" => {
                let store = child
                    .child_text(Page::ItemOperations, "Store")
                    .unwrap_or_default();
                if !store.eq_ignore_ascii_case("Mailbox") {
                    op("Fetch", status::STORE_UNKNOWN)
                } else if let Some(r) = child.child_text(Page::AirSyncBase, "FileReference") {
                    fetch_attachment(ctx, &r).await?
                } else if child.child(Page::AirSync, "ServerId").is_some() {
                    fetch_item(ctx, child).await?
                } else {
                    // LongId (search results) comes with Search.
                    op("Fetch", status::ACTION_NOT_SUPPORTED)
                }
            }
            "EmptyFolderContents" => empty_folder(ctx, child).await?,
            other => {
                tracing::debug!(operation = other, "unsupported ItemOperations operation");
                Element {
                    tag: child.tag,
                    children: vec![Node::Element(
                        Element::new(Page::ItemOperations, "Status")
                            .with_text(status::ACTION_NOT_SUPPORTED.to_string()),
                    )],
                }
            }
        };
        response.push(el);
    }
    Ok(wbxml(
        &op("ItemOperations", status::SUCCESS).with_child(response),
    ))
}

/// `GetAttachment`: the attachment as the raw response body. The request
/// line's `AttachmentName` is the `FileReference` from the item.
pub async fn get_attachment(ctx: &Ctx<'_>) -> HandlerResult {
    let reference = ctx.line.attachment_name.as_deref().unwrap_or_default();
    let Some((email_id, blob_id)) = parse_file_reference(reference) else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    let Some((ctype, _, size)) = attachment_info(ctx, email_id, blob_id).await? else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    if size > MAX_ATTACHMENT {
        return Ok(StatusCode::PAYLOAD_TOO_LARGE.into_response());
    }
    let data = ctx.jmap.download(blob_id, "attachment").await?;
    Ok((StatusCode::OK, [(header::CONTENT_TYPE, ctype)], data).into_response())
}
