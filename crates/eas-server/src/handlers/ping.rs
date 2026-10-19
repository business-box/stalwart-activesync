//! `Ping` (MS-ASCMD 2.2.1.13): hold the request until one of the listed
//! folders has something to sync, or the heartbeat runs out.

use super::folder_sync::KIND_MAILBOX;
use super::sync::server_diff;
use super::{wbxml, Ctx, HandlerResult};
use eas_state::Item;
use eas_wbxml::{Element, Page};
use std::collections::{HashMap, HashSet};
use std::time::Duration;
use tokio::time::Instant;

/// Ping status codes (MS-ASCMD 2.2.3.177.11).
mod status {
    pub const EXPIRED: u8 = 1;
    pub const CHANGES: u8 = 2;
    pub const MISSING_PARAMETERS: u8 = 3;
    pub const SYNTAX_ERROR: u8 = 4;
    pub const BAD_HEARTBEAT: u8 = 5;
    pub const TOO_MANY_FOLDERS: u8 = 6;
    pub const FOLDER_SYNC_REQUIRED: u8 = 7;
}

pub const MIN_HEARTBEAT: u64 = 60;
pub const MAX_HEARTBEAT: u64 = 3540;
const MAX_FOLDERS: usize = 300;
/// Recheck even without a push event, in case the stream is down.
const RECHECK: Duration = Duration::from_secs(600);
/// Several state changes usually arrive for one delivery; let them settle.
const SETTLE: Duration = Duration::from_millis(500);

fn reply(code: u8) -> Element {
    Element::new(Page::Ping, "Ping").with_text_child("Status", code.to_string())
}

/// Folders with pending changes, or `Err(())` if one is unknown and the
/// device must run `FolderSync` first.
async fn pending(
    ctx: &Ctx<'_>,
    folders: &[String],
) -> Result<Result<Vec<String>, ()>, super::HandlerError> {
    let store = &ctx.state.store;
    let dev = ctx.device.id;
    let mut out = Vec::new();
    for id in folders {
        let folder = match store.folder(dev, id).await? {
            Some(f) if f.kind == KIND_MAILBOX => f,
            _ => return Ok(Err(())),
        };
        // A folder the device never synced has nothing to report yet.
        let Some(state) = store.sync_state(dev, id).await? else {
            continue;
        };
        let items: HashMap<String, Item> = store
            .items(dev, id)
            .await?
            .into_iter()
            .map(|i| (i.jmap_id.clone(), i))
            .collect();
        let diff = server_diff(
            ctx,
            &folder,
            state.jmap_state.as_deref(),
            &items,
            state.filter_type,
            &HashSet::new(),
        )
        .await?;
        if diff.total() > 0 {
            out.push(id.clone());
        }
    }
    Ok(Ok(out))
}

pub async fn handle(ctx: &Ctx<'_>, request: Option<Element>) -> HandlerResult {
    let store = &ctx.state.store;
    let dev = ctx.device.id;
    let saved = store.ping_state(dev).await?;

    let (mut heartbeat, mut folders) = (None, None);
    if let Some(req) = &request {
        if !req.is(Page::Ping, "Ping") {
            return Ok(wbxml(&reply(status::SYNTAX_ERROR)));
        }
        if let Some(h) = req.child_text(Page::Ping, "HeartbeatInterval") {
            match h.trim().parse::<u64>() {
                Ok(v) => heartbeat = Some(v),
                Err(_) => return Ok(wbxml(&reply(status::SYNTAX_ERROR))),
            }
        }
        if let Some(fs) = req.child(Page::Ping, "Folders") {
            let ids: Vec<String> = fs
                .children_named(Page::Ping, "Folder")
                .filter_map(|f| f.child_text(Page::Ping, "Id"))
                .map(|id| id.trim().to_string())
                .collect();
            folders = Some(ids);
        }
    }
    // Whatever the request leaves out comes from the last Ping.
    let (Some(heartbeat), Some(folders)) = (
        heartbeat.or(saved.as_ref().map(|s| s.0)),
        folders.or(saved.map(|s| s.1)),
    ) else {
        return Ok(wbxml(&reply(status::MISSING_PARAMETERS)));
    };
    if folders.is_empty() {
        return Ok(wbxml(&reply(status::MISSING_PARAMETERS)));
    }
    if !(MIN_HEARTBEAT..=MAX_HEARTBEAT).contains(&heartbeat) {
        let allowed = heartbeat.clamp(MIN_HEARTBEAT, MAX_HEARTBEAT);
        return Ok(wbxml(
            &reply(status::BAD_HEARTBEAT).with_text_child("HeartbeatInterval", allowed.to_string()),
        ));
    }
    if folders.len() > MAX_FOLDERS {
        return Ok(wbxml(
            &reply(status::TOO_MANY_FOLDERS).with_text_child("MaxFolders", MAX_FOLDERS.to_string()),
        ));
    }
    store.set_ping_state(dev, heartbeat, &folders).await?;

    // Subscribe before the first check so nothing slips in between.
    let slot = ctx.state.pings.take(dev);
    let superseded = slot.notified();
    tokio::pin!(superseded);
    superseded.as_mut().enable();
    let feed = ctx.state.push.subscribe(&ctx.jmap);
    let mut rx = feed.receiver();

    let deadline = Instant::now() + Duration::from_secs(heartbeat);
    loop {
        match pending(ctx, &folders).await? {
            Err(()) => return Ok(wbxml(&reply(status::FOLDER_SYNC_REQUIRED))),
            Ok(changed) if !changed.is_empty() => {
                let mut fs = Element::new(Page::Ping, "Folders");
                for id in changed {
                    fs.push(Element::new(Page::Ping, "Folder").with_text(id));
                }
                return Ok(wbxml(&reply(status::CHANGES).with_child(fs)));
            }
            Ok(_) => {}
        }
        let recheck = (Instant::now() + RECHECK).min(deadline);
        tokio::select! {
            _ = tokio::time::sleep_until(deadline) => {
                return Ok(wbxml(&reply(status::EXPIRED)));
            }
            _ = &mut superseded => {
                return Ok(wbxml(&reply(status::EXPIRED)));
            }
            r = rx.changed() => {
                if r.is_err() {
                    // Feed gone: fall back to the periodic recheck.
                    tokio::time::sleep_until(recheck).await;
                } else {
                    tokio::time::sleep(SETTLE).await;
                    rx.borrow_and_update();
                }
            }
            _ = tokio::time::sleep_until(recheck) => {}
        }
        if Instant::now() >= deadline {
            return Ok(wbxml(&reply(status::EXPIRED)));
        }
    }
}
