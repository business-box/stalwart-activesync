//! `GetItemEstimate`: how many changes a `Sync` would deliver per folder.

use super::classes::Class;
use super::sync::{parse_options, server_diff};
use super::{wbxml, Ctx, HandlerError, HandlerResult};
use eas_state::Item;
use eas_wbxml::{Element, Page};
use std::collections::{HashMap, HashSet};

/// GetItemEstimate status codes (MS-ASCMD 2.2.3.177.6).
mod status {
    pub const SUCCESS: u8 = 1;
    pub const INVALID_COLLECTION: u8 = 2;
    pub const NOT_PRIMED: u8 = 3;
    pub const INVALID_SYNC_KEY: u8 = 4;
}

fn response(code: u8, cid: &str, estimate: Option<usize>) -> Element {
    let mut coll =
        Element::new(Page::GetItemEstimate, "Collection").with_text_child("CollectionId", cid);
    if let Some(n) = estimate {
        coll = coll.with_text_child("Estimate", n.to_string());
    }
    Element::new(Page::GetItemEstimate, "Response")
        .with_text_child("Status", code.to_string())
        .with_child(coll)
}

pub async fn handle(ctx: &Ctx<'_>, request: Option<Element>) -> HandlerResult {
    let req = request.ok_or(HandlerError::BadRequest("empty GetItemEstimate"))?;
    let store = &ctx.state.store;
    let dev = ctx.device.id;
    let mut out = Element::new(Page::GetItemEstimate, "GetItemEstimate");
    for c in req
        .child(Page::GetItemEstimate, "Collections")
        .into_iter()
        .flat_map(|cs| cs.children_named(Page::GetItemEstimate, "Collection"))
    {
        let cid = c
            .child_text(Page::GetItemEstimate, "CollectionId")
            .unwrap_or_default();
        // 14.0 and later put SyncKey and Options on the AirSync page.
        let key = c.child_text(Page::AirSync, "SyncKey").unwrap_or_default();
        let folder = match store.folder(dev, &cid).await? {
            Some(f) if Class::of(&f).is_some() => f,
            _ => {
                out.push(response(status::INVALID_COLLECTION, &cid, None));
                continue;
            }
        };
        if key.trim() == "0" {
            out.push(response(status::NOT_PRIMED, &cid, None));
            continue;
        }
        let Some(state) = store.sync_state(dev, &cid).await? else {
            out.push(response(status::NOT_PRIMED, &cid, None));
            continue;
        };
        if state.sync_key != key.trim() {
            out.push(response(status::INVALID_SYNC_KEY, &cid, None));
            continue;
        }
        let filter = match c.child(Page::AirSync, "Options") {
            Some(o) => parse_options(Some(o)).0,
            None => state.filter_type,
        };
        let items: HashMap<String, Item> = store
            .items(dev, &cid)
            .await?
            .into_iter()
            .map(|i| (i.jmap_id.clone(), i))
            .collect();
        let diff = server_diff(
            ctx,
            &folder,
            state.jmap_state.as_deref(),
            &items,
            filter,
            &HashSet::new(),
        )
        .await?;
        out.push(response(status::SUCCESS, &cid, Some(diff.total())));
    }
    Ok(wbxml(&out))
}
