//! `MoveItems` (MS-ASCMD 2.2.1.12): move emails between folders.

use super::folder_sync::KIND_MAILBOX;
use super::sync::server_id;
use super::{wbxml, Ctx, HandlerError, HandlerResult};
use eas_convert::email::{state_hash, HASH_PROPERTIES};
use eas_state::Item;
use eas_wbxml::{Element, Page};
use serde_json::{json, Map, Value};

/// MoveItems status codes (MS-ASCMD 2.2.3.177.9).
mod status {
    pub const INVALID_SOURCE: u8 = 1;
    pub const INVALID_DESTINATION: u8 = 2;
    pub const SUCCESS: u8 = 3;
    pub const SAME_FOLDER: u8 = 4;
    pub const FAILED: u8 = 5;
}

fn response(src: &str, code: u8, dst: Option<String>) -> Element {
    let mut r = Element::new(Page::Move, "Response")
        .with_text_child("SrcMsgId", src)
        .with_text_child("Status", code.to_string());
    if let Some(d) = dst {
        r = r.with_text_child("DstMsgId", d);
    }
    r
}

pub async fn handle(ctx: &Ctx<'_>, request: Option<Element>) -> HandlerResult {
    let req = request.ok_or(HandlerError::BadRequest("empty MoveItems"))?;
    if !req.is(Page::Move, "MoveItems") {
        return Err(HandlerError::BadRequest("not a MoveItems request"));
    }
    let store = &ctx.state.store;
    let dev = ctx.device.id;
    let _guard = ctx.state.locks.get(dev).lock_owned().await;
    let mut out = Element::new(Page::Move, "MoveItems");

    for mv in req.children_named(Page::Move, "Move") {
        let src_msg = mv.child_text(Page::Move, "SrcMsgId").unwrap_or_default();
        let src_fld = mv.child_text(Page::Move, "SrcFldId").unwrap_or_default();
        let dst_fld = mv.child_text(Page::Move, "DstFldId").unwrap_or_default();
        if src_fld == dst_fld {
            out.push(response(&src_msg, status::SAME_FOLDER, None));
            continue;
        }
        let src = match store.folder(dev, &src_fld).await? {
            Some(f) if f.kind == KIND_MAILBOX => f,
            _ => {
                out.push(response(&src_msg, status::INVALID_SOURCE, None));
                continue;
            }
        };
        let dst = match store.folder(dev, &dst_fld).await? {
            Some(f) if f.kind == KIND_MAILBOX => f,
            _ => {
                out.push(response(&src_msg, status::INVALID_DESTINATION, None));
                continue;
            }
        };
        let Some(jmap_id) = src_msg
            .split_once(':')
            .filter(|(f, _)| *f == src_fld)
            .map(|(_, id)| id.to_string())
        else {
            out.push(response(&src_msg, status::INVALID_SOURCE, None));
            continue;
        };

        let mut update = Map::new();
        update.insert(
            jmap_id.clone(),
            json!({
                format!("mailboxIds/{}", src.jmap_id): null,
                format!("mailboxIds/{}", dst.jmap_id): true,
            }),
        );
        let failed = ctx.jmap.email_set(update, Vec::new()).await?;
        if let Some((_, kind)) = failed.first() {
            let code = if kind == "notFound" {
                status::INVALID_SOURCE
            } else {
                status::FAILED
            };
            out.push(response(&src_msg, code, None));
            continue;
        }

        let dst_id = server_id(&dst_fld, &jmap_id);
        // The device already shows the item in the destination; tell the
        // destination's sync state so it isn't added a second time. A
        // destination never synced picks it up in its first Sync instead.
        let dst_item = if store.sync_state(dev, &dst_fld).await?.is_some() {
            let props = json!({ "properties": HASH_PROPERTIES });
            ctx.jmap
                .email_get(std::slice::from_ref(&jmap_id), &props)
                .await?
                .first()
                .map(|e: &Value| Item {
                    server_id: dst_id.clone(),
                    jmap_id: jmap_id.clone(),
                    hash: state_hash(e),
                })
        } else {
            None
        };
        store
            .move_item(dev, &src_fld, &src_msg, &dst_fld, dst_item)
            .await?;
        out.push(response(&src_msg, status::SUCCESS, Some(dst_id)));
    }
    Ok(wbxml(&out))
}
