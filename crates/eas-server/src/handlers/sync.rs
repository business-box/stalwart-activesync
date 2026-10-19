//! `Sync` for mail folders (MS-ASCMD 2.2.1.21).
//!
//! Per collection the device sends its SyncKey; the gateway:
//!
//! 1. answers SyncKey 0 with a fresh key and nothing else;
//! 2. replays the stored response when the device retries with the
//!    previous key;
//! 3. otherwise applies the device's changes (read, flag, delete, fetch),
//!    then works out the server's changes by comparing the mailbox listing
//!    with the items the device holds, plus `Email/changes` for read and flag
//!    state, and sends at most a window of them.
//!
//! The items table always matches what the device was last told, so
//! anything left over from a full window is found again next time.

use super::folder_sync::{folder_type, KIND_MAILBOX};
use super::{empty, sync_key, wbxml, Ctx, HandlerError, HandlerResult};
use eas_convert::email::{
    self, mime_truncation_size, BodyOptions, BodyPreference, BodyType, HASH_PROPERTIES,
};
use eas_state::{Folder, Item, SyncCommit, SyncState};
use eas_wbxml::{Element, Page};
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use std::time::Duration;

/// Sync status codes (MS-ASCMD 2.2.3.177.16).
pub mod status {
    pub const SUCCESS: u8 = 1;
    pub const INVALID_SYNC_KEY: u8 = 3;
    pub const PROTOCOL_ERROR: u8 = 4;
    pub const SERVER_ERROR: u8 = 5;
    pub const CONVERSION_ERROR: u8 = 6;
    pub const NOT_FOUND: u8 = 8;
    pub const HIERARCHY_CHANGED: u8 = 12;
    pub const INCOMPLETE_REQUEST: u8 = 13;
    pub const INVALID_WAIT: u8 = 14;
}

const DEFAULT_WINDOW: usize = 100;
const MAX_WINDOW: usize = 512;
/// Recheck a waiting `Sync` even without a push event, in case the
/// stream is down.
const WAIT_RECHECK: Duration = Duration::from_secs(600);
/// Longest wait accepted, in seconds (MS-ASCMD: 59 minutes).
const MAX_WAIT_SECS: u64 = 59 * 60;

#[derive(Debug, Clone)]
pub struct CollectionRequest {
    pub sync_key: String,
    pub collection_id: String,
    pub get_changes: Option<bool>,
    pub deletes_as_moves: Option<bool>,
    pub window_size: Option<usize>,
    pub options: Option<Element>,
    pub commands: Vec<Element>,
}

#[derive(Debug, Clone)]
pub struct SyncRequest {
    pub collections: Vec<CollectionRequest>,
    pub window_size: Option<usize>,
    /// Seconds to hold the request open when nothing changed.
    pub wait: Option<u64>,
}

fn flag_value(el: &Element, name: &str) -> Option<bool> {
    // Empty element means true; "0" means false.
    el.child(Page::AirSync, name)
        .map(|c| c.text().map(|t| t.trim() != "0").unwrap_or(true))
}

fn number<T: std::str::FromStr>(el: &Element, page: Page, name: &str) -> Option<T> {
    el.child_text(page, name)
        .and_then(|t| t.trim().parse().ok())
}

pub fn parse_request(req: &Element) -> Result<SyncRequest, u8> {
    if !req.is(Page::AirSync, "Sync") {
        return Err(status::PROTOCOL_ERROR);
    }
    let mut collections = Vec::new();
    for c in req
        .child(Page::AirSync, "Collections")
        .into_iter()
        .flat_map(|cs| cs.children_named(Page::AirSync, "Collection"))
    {
        let sync_key = c
            .child_text(Page::AirSync, "SyncKey")
            .ok_or(status::PROTOCOL_ERROR)?
            .trim()
            .to_string();
        let collection_id = c
            .child_text(Page::AirSync, "CollectionId")
            .ok_or(status::PROTOCOL_ERROR)?
            .trim()
            .to_string();
        let commands = c
            .child(Page::AirSync, "Commands")
            .map(|cmds| cmds.elements().cloned().collect())
            .unwrap_or_default();
        // 16.x may send one Options per class; mail folders only have one.
        let options = c
            .children_named(Page::AirSync, "Options")
            .find(|o| {
                o.child_text(Page::AirSync, "Class")
                    .map_or(true, |cl| cl == "Email")
            })
            .cloned();
        collections.push(CollectionRequest {
            sync_key,
            collection_id,
            get_changes: flag_value(c, "GetChanges"),
            deletes_as_moves: flag_value(c, "DeletesAsMoves"),
            window_size: number(c, Page::AirSync, "WindowSize"),
            options,
            commands,
        });
    }
    let wait = match (
        number::<u64>(req, Page::AirSync, "Wait"),
        number::<u64>(req, Page::AirSync, "HeartbeatInterval"),
    ) {
        (Some(m), _) if m == 0 || m * 60 > MAX_WAIT_SECS => return Err(status::INVALID_WAIT),
        (Some(m), _) => Some(m * 60),
        (None, Some(s)) if !(60..=MAX_WAIT_SECS).contains(&s) => return Err(status::INVALID_WAIT),
        (None, s) => s,
    };
    Ok(SyncRequest {
        collections,
        window_size: number(req, Page::AirSync, "WindowSize"),
        wait,
    })
}

/// Filter type and body options from an `Options` element.
pub fn parse_options(options: Option<&Element>) -> (u8, BodyOptions) {
    let Some(o) = options else {
        return (0, BodyOptions::default());
    };
    let filter = number(o, Page::AirSync, "FilterType").unwrap_or(0);
    let preferences = o
        .children_named(Page::AirSyncBase, "BodyPreference")
        .filter_map(|p| {
            Some(BodyPreference {
                body_type: BodyType::from_code(number(p, Page::AirSyncBase, "Type")?)?,
                truncation_size: number(p, Page::AirSyncBase, "TruncationSize"),
                all_or_none: p
                    .child_text(Page::AirSyncBase, "AllOrNone")
                    .is_some_and(|v| v.trim() == "1"),
                preview: number(p, Page::AirSyncBase, "Preview"),
            })
        })
        .collect();
    let body = BodyOptions {
        preferences,
        mime_support: number(o, Page::AirSync, "MIMESupport").unwrap_or(0),
        mime_truncation: number::<u8>(o, Page::AirSync, "MIMETruncation")
            .and_then(mime_truncation_size),
    };
    (filter, body)
}

/// Lower bound for `receivedAt` per `FilterType` (MS-ASCMD 2.2.3.69.2).
pub fn filter_after(filter: u8) -> Option<String> {
    let days = match filter {
        1 => 1,
        2 => 3,
        3 => 7,
        4 => 14,
        5 => 30,
        6 => 90,
        7 => 180,
        _ => return None,
    };
    let t = chrono::Utc::now() - chrono::Duration::days(days);
    Some(t.format("%Y-%m-%dT%H:%M:%SZ").to_string())
}

pub(super) fn server_id(folder: &str, jmap_id: &str) -> String {
    format!("{folder}:{jmap_id}")
}

fn collection_status(key: &str, cid: &str, code: u8) -> Element {
    Element::new(Page::AirSync, "Collection")
        .with_text_child("SyncKey", key)
        .with_text_child("CollectionId", cid)
        .with_text_child("Status", code.to_string())
}

fn top_status(code: u8) -> Element {
    Element::new(Page::AirSync, "Sync").with_text_child("Status", code.to_string())
}

/// What the server has to tell the device about one folder.
#[derive(Debug, Default)]
pub struct Diff {
    pub deletes: Vec<String>,
    /// Server ID and the email with its current keywords.
    pub changes: Vec<(String, Value)>,
    /// JMAP IDs, newest first.
    pub adds: Vec<String>,
    /// JMAP `Email` state to store once every change has been sent.
    pub new_state: String,
}

impl Diff {
    pub fn total(&self) -> usize {
        self.deletes.len() + self.changes.len() + self.adds.len()
    }
}

/// Compares the mailbox with what the device holds. Items in `skip` were
/// just changed by the device and need no echo.
pub async fn server_diff(
    ctx: &Ctx<'_>,
    folder: &Folder,
    since: Option<&str>,
    items: &HashMap<String, Item>,
    filter: u8,
    skip: &HashSet<String>,
) -> Result<Diff, HandlerError> {
    // Changes first, so nothing between them and the listing is lost.
    let (changed, new_state) = match since {
        Some(s) => match ctx.jmap.email_changes(s).await {
            Ok(c) => {
                let set: HashSet<String> = c.updated.into_iter().chain(c.created).collect();
                (Some(set), c.new_state)
            }
            Err(e) if e.is_method("cannotCalculateChanges") => {
                tracing::info!(device = %ctx.line.device_id, folder = %folder.server_id, "JMAP state too old, comparing all flags");
                (None, ctx.jmap.email_state().await?)
            }
            Err(e) => return Err(e.into()),
        },
        None => (None, ctx.jmap.email_state().await?),
    };
    let after = filter_after(filter);
    let current = ctx
        .jmap
        .email_ids(&folder.jmap_id, after.as_deref())
        .await?;
    let current_set: HashSet<&str> = current.iter().map(String::as_str).collect();

    let mut diff = Diff {
        new_state,
        ..Default::default()
    };
    diff.adds = current
        .iter()
        .filter(|id| !items.contains_key(*id))
        .cloned()
        .collect();
    let mut candidates = Vec::new();
    for (jmap_id, item) in items {
        if !current_set.contains(jmap_id.as_str()) {
            diff.deletes.push(item.server_id.clone());
        } else if !skip.contains(jmap_id) && changed.as_ref().map_or(true, |c| c.contains(jmap_id))
        {
            candidates.push(jmap_id.clone());
        }
    }
    diff.deletes.sort();
    if !candidates.is_empty() {
        let props = json!({ "properties": HASH_PROPERTIES });
        for e in ctx.jmap.email_get(&candidates, &props).await? {
            let id = e.get("id").and_then(Value::as_str).unwrap_or_default();
            if let Some(item) = items.get(id) {
                if email::state_hash(&e) != item.hash {
                    diff.changes.push((item.server_id.clone(), e));
                }
            }
        }
        diff.changes.sort_by(|a, b| a.0.cmp(&b.0));
    }
    Ok(diff)
}

/// Full `ApplicationData` for each email in `ids`, downloading the message
/// when the device wants MIME. Emails that vanished meanwhile are skipped.
pub(super) async fn serialise(
    ctx: &Ctx<'_>,
    ids: &[String],
    opts: &BodyOptions,
) -> Result<Vec<(String, Value, Element)>, HandlerError> {
    let emails = ctx.jmap.email_get(ids, &opts.get_args()).await?;
    let mut by_id: HashMap<String, Value> = emails
        .into_iter()
        .filter_map(|e| Some((e.get("id")?.as_str()?.to_string(), e)))
        .collect();
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        let Some(e) = by_id.remove(id) else { continue };
        let mime = if opts.needs_mime() {
            let blob = e.get("blobId").and_then(Value::as_str).unwrap_or_default();
            Some(ctx.jmap.download(blob, "message.eml").await?)
        } else {
            None
        };
        let data = email::application_data(&e, opts, mime.as_deref());
        out.push((id.clone(), e, data));
    }
    Ok(out)
}

pub async fn handle(ctx: &Ctx<'_>, request: Option<Element>) -> HandlerResult {
    let store = &ctx.state.store;
    let dev = ctx.device.id;
    let cached = request.is_none();
    let req = match request {
        Some(r) => r,
        None => match ctx
            .device
            .last_sync_request
            .as_deref()
            .and_then(|b| eas_wbxml::decode(b).ok())
        {
            Some(r) => r,
            None => return Ok(wbxml(&top_status(status::INCOMPLETE_REQUEST))),
        },
    };
    let mut parsed = match parse_request(&req) {
        Ok(p) => p,
        Err(code) => return Ok(wbxml(&top_status(code))),
    };
    if cached {
        // An empty request repeats the last one at the current keys.
        for c in &mut parsed.collections {
            if let Some(s) = store.sync_state(dev, &c.collection_id).await? {
                c.sync_key = s.sync_key;
            }
        }
    } else {
        // Cache without client commands, which must never run twice.
        let mut stripped = req.clone();
        strip_commands(&mut stripped);
        store
            .set_last_sync_request(dev, &eas_wbxml::encode(&stripped))
            .await?;
    }
    if parsed.collections.is_empty() {
        return Ok(wbxml(&top_status(status::INCOMPLETE_REQUEST)));
    }

    let deadline = parsed
        .wait
        .map(|s| tokio::time::Instant::now() + Duration::from_secs(s));
    // Subscribe before the first pass so a change during it still wakes us.
    let feed = deadline.map(|_| ctx.state.push.subscribe(&ctx.jmap));
    let mut rx = feed.as_ref().map(|f| f.receiver());
    let mut first = true;
    loop {
        let response = {
            let _guard = ctx.state.locks.get(dev).lock_owned().await;
            let mut out = Vec::new();
            for coll in &parsed.collections {
                if let Some(el) = sync_collection(ctx, coll, parsed.window_size, first).await? {
                    out.push(el);
                }
            }
            out
        };
        if !response.is_empty() {
            let mut cols = Element::new(Page::AirSync, "Collections");
            for c in response {
                cols.push(c);
            }
            return Ok(wbxml(&Element::new(Page::AirSync, "Sync").with_child(cols)));
        }
        let (Some(deadline), Some(rx)) = (deadline, rx.as_mut()) else {
            return Ok(empty());
        };
        // Nothing changed: hold the request until push says mail moved, the
        // periodic recheck comes round, or the wait runs out.
        first = false;
        let recheck = (tokio::time::Instant::now() + WAIT_RECHECK).min(deadline);
        tokio::select! {
            r = rx.changed() => {
                if r.is_err() {
                    tokio::time::sleep_until(recheck).await;
                } else {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    rx.borrow_and_update();
                }
            }
            _ = tokio::time::sleep_until(recheck) => {}
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(empty());
        }
    }
}

fn strip_commands(req: &mut Element) {
    for n in &mut req.children {
        if let eas_wbxml::Node::Element(cols) = n {
            if cols.is(Page::AirSync, "Collections") {
                for c in &mut cols.children {
                    if let eas_wbxml::Node::Element(coll) = c {
                        coll.children.retain(|x| {
                            !matches!(x, eas_wbxml::Node::Element(e) if e.is(Page::AirSync, "Commands"))
                        });
                    }
                }
            }
        }
    }
}

/// One collection of a Sync request. `None` when there is nothing to say,
/// so the collection is left out of the response. After the first round of
/// a waiting request, `first` is false: client commands were applied then,
/// and the keys to use are the stored ones.
async fn sync_collection(
    ctx: &Ctx<'_>,
    coll: &CollectionRequest,
    global_window: Option<usize>,
    first: bool,
) -> Result<Option<Element>, HandlerError> {
    let store = &ctx.state.store;
    let dev = ctx.device.id;
    let cid = coll.collection_id.as_str();
    let folder = match store.folder(dev, cid).await? {
        Some(f) if f.kind == KIND_MAILBOX => f,
        _ => {
            return Ok(Some(collection_status(
                &coll.sync_key,
                cid,
                status::HIERARCHY_CHANGED,
            )))
        }
    };
    let state = store.sync_state(dev, cid).await?;

    let mut key = coll.sync_key.clone();
    if !first {
        match &state {
            Some(s) => key = s.sync_key.clone(),
            None => return Ok(None),
        }
    }

    if key == "0" {
        let options = coll.options.as_ref();
        let (filter, _) = parse_options(options);
        let new_key = sync_key(1);
        let resp = collection_status(&new_key, cid, status::SUCCESS);
        store
            .commit_sync(
                dev,
                SyncCommit {
                    folder: cid.to_string(),
                    reset: true,
                    state: SyncState {
                        sync_key: new_key,
                        prev_key: None,
                        prev_response: None,
                        counter: 1,
                        jmap_state: None,
                        filter_type: filter,
                        options: options.map(eas_wbxml::encode),
                    },
                    upsert: Vec::new(),
                    delete: Vec::new(),
                },
            )
            .await?;
        return Ok(Some(resp));
    }

    let Some(state) = state else {
        return Ok(Some(collection_status(&key, cid, status::INVALID_SYNC_KEY)));
    };
    if state.sync_key != key {
        if state.prev_key.as_deref() == Some(key.as_str()) {
            if let Some(prev) = state
                .prev_response
                .as_deref()
                .and_then(|b| eas_wbxml::decode(b).ok())
            {
                tracing::info!(device = %ctx.line.device_id, folder = cid, "Sync retry, replaying");
                return Ok(Some(prev));
            }
        }
        return Ok(Some(collection_status(&key, cid, status::INVALID_SYNC_KEY)));
    }

    let stored_options = state
        .options
        .as_deref()
        .and_then(|b| eas_wbxml::decode(b).ok());
    let options = coll.options.clone().or(stored_options);
    let (filter, body_opts) = parse_options(options.as_ref());
    let window = coll
        .window_size
        .or(global_window)
        .unwrap_or(DEFAULT_WINDOW)
        .clamp(1, MAX_WINDOW);

    let mut items: HashMap<String, Item> = store
        .items(dev, cid)
        .await?
        .into_iter()
        .map(|i| (i.jmap_id.clone(), i))
        .collect();
    let by_server_id: HashMap<String, String> = items
        .values()
        .map(|i| (i.server_id.clone(), i.jmap_id.clone()))
        .collect();

    let mut responses: Vec<Element> = Vec::new();
    let mut upsert: Vec<Item> = Vec::new();
    let mut delete: Vec<String> = Vec::new();
    let mut touched: HashSet<String> = HashSet::new();
    let had_commands = first && !coll.commands.is_empty();

    if first {
        apply_client_commands(
            ctx,
            coll,
            &folder,
            &body_opts,
            &mut items,
            &by_server_id,
            &mut responses,
            &mut upsert,
            &mut delete,
            &mut touched,
        )
        .await?;
    }

    let mut commands: Vec<Element> = Vec::new();
    let mut more = false;
    let mut jmap_state = state.jmap_state.clone();
    if coll.get_changes.unwrap_or(true) {
        let diff = server_diff(
            ctx,
            &folder,
            state.jmap_state.as_deref(),
            &items,
            filter,
            &touched,
        )
        .await?;
        more = diff.total() > window;
        let mut room = window;
        for sid in diff.deletes.iter().take(room) {
            commands.push(
                Element::new(Page::AirSync, "Delete").with_text_child("ServerId", sid.clone()),
            );
            delete.push(sid.clone());
        }
        room -= diff.deletes.len().min(room);
        let n_changes = diff.changes.len().min(room);
        for (sid, e) in diff.changes.iter().take(room) {
            commands.push(
                Element::new(Page::AirSync, "Change")
                    .with_text_child("ServerId", sid.clone())
                    .with_child(email::change_data(e)),
            );
            let jmap_id = e.get("id").and_then(Value::as_str).unwrap_or_default();
            upsert.push(Item {
                server_id: sid.clone(),
                jmap_id: jmap_id.to_string(),
                hash: email::state_hash(e),
            });
        }
        room -= n_changes;
        if n_changes == diff.changes.len() {
            jmap_state = Some(diff.new_state.clone());
        }
        let add_ids: Vec<String> = diff.adds.iter().take(room).cloned().collect();
        for (id, e, data) in serialise(ctx, &add_ids, &body_opts).await? {
            let sid = server_id(cid, &id);
            commands.push(
                Element::new(Page::AirSync, "Add")
                    .with_text_child("ServerId", sid.clone())
                    .with_child(data),
            );
            upsert.push(Item {
                server_id: sid,
                jmap_id: id,
                hash: email::state_hash(&e),
            });
        }
    }

    let options_changed = coll.options.is_some()
        && coll.options
            != state
                .options
                .as_deref()
                .and_then(|b| eas_wbxml::decode(b).ok());
    if !had_commands && commands.is_empty() && !more {
        if options_changed || jmap_state != state.jmap_state {
            // Nothing for the device, but remember the new state and options
            // without moving the key.
            store
                .commit_sync(
                    dev,
                    SyncCommit {
                        folder: cid.to_string(),
                        reset: false,
                        state: SyncState {
                            jmap_state,
                            filter_type: filter,
                            options: options.as_ref().map(eas_wbxml::encode),
                            ..state
                        },
                        upsert: Vec::new(),
                        delete: Vec::new(),
                    },
                )
                .await?;
        }
        return Ok(None);
    }

    let counter = state.counter + 1;
    let new_key = sync_key(counter);
    let mut resp = collection_status(&new_key, cid, status::SUCCESS);
    if !responses.is_empty() {
        let mut r = Element::new(Page::AirSync, "Responses");
        for x in responses {
            r.push(x);
        }
        resp.push(r);
    }
    if more {
        resp.push(Element::new(Page::AirSync, "MoreAvailable"));
    }
    if !commands.is_empty() {
        let mut c = Element::new(Page::AirSync, "Commands");
        for x in commands {
            c.push(x);
        }
        resp.push(c);
    }
    store
        .commit_sync(
            dev,
            SyncCommit {
                folder: cid.to_string(),
                reset: false,
                state: SyncState {
                    sync_key: new_key,
                    prev_key: Some(key),
                    prev_response: Some(eas_wbxml::encode(&resp)),
                    counter,
                    jmap_state,
                    filter_type: filter,
                    options: options.as_ref().map(eas_wbxml::encode),
                },
                upsert,
                delete,
            },
        )
        .await?;
    Ok(Some(resp))
}

/// Applies `Change`, `Delete`, `Fetch` and `Add` from the device.
#[allow(clippy::too_many_arguments)]
async fn apply_client_commands(
    ctx: &Ctx<'_>,
    coll: &CollectionRequest,
    folder: &Folder,
    body_opts: &BodyOptions,
    items: &mut HashMap<String, Item>,
    by_server_id: &HashMap<String, String>,
    responses: &mut Vec<Element>,
    upsert: &mut Vec<Item>,
    delete: &mut Vec<String>,
    touched: &mut HashSet<String>,
) -> Result<(), HandlerError> {
    let mut updates = Map::new();
    let mut destroy = Vec::new();
    let mut fetch: Vec<(String, String)> = Vec::new();
    let mut change_sids: HashMap<String, String> = HashMap::new();

    let trash = if coll.deletes_as_moves.unwrap_or(true) && folder.folder_type != folder_type::TRASH
    {
        ctx.state
            .store
            .folders(ctx.device.id)
            .await?
            .into_iter()
            .find(|f| f.kind == KIND_MAILBOX && f.folder_type == folder_type::TRASH)
            .map(|f| f.jmap_id)
    } else {
        None
    };

    for cmd in &coll.commands {
        let sid = cmd
            .child_text(Page::AirSync, "ServerId")
            .unwrap_or_default();
        match cmd.name() {
            "Change" => {
                let Some(jmap_id) = by_server_id.get(&sid) else {
                    responses.push(
                        Element::new(Page::AirSync, "Change")
                            .with_text_child("ServerId", sid)
                            .with_text_child("Status", status::NOT_FOUND.to_string()),
                    );
                    continue;
                };
                let patch = cmd
                    .child(Page::AirSync, "ApplicationData")
                    .map(email::change_patch)
                    .unwrap_or_default();
                if !patch.is_empty() {
                    updates.insert(jmap_id.clone(), Value::Object(patch));
                    change_sids.insert(jmap_id.clone(), sid);
                }
                touched.insert(jmap_id.clone());
            }
            "Delete" => {
                let Some(jmap_id) = by_server_id.get(&sid) else {
                    continue;
                };
                match &trash {
                    Some(t) => {
                        updates.insert(
                            jmap_id.clone(),
                            json!({
                                format!("mailboxIds/{}", folder.jmap_id): null,
                                format!("mailboxIds/{t}"): true,
                            }),
                        );
                    }
                    None => destroy.push(jmap_id.clone()),
                }
                items.remove(jmap_id);
                delete.push(sid);
            }
            "Fetch" => match by_server_id.get(&sid) {
                Some(jmap_id) => fetch.push((sid, jmap_id.clone())),
                None => responses.push(
                    Element::new(Page::AirSync, "Fetch")
                        .with_text_child("ServerId", sid)
                        .with_text_child("Status", status::NOT_FOUND.to_string()),
                ),
            },
            "Add" => {
                // Mail items can't be created by Sync before 16.0 drafts,
                // which are not supported yet.
                let client_id = cmd
                    .child_text(Page::AirSync, "ClientId")
                    .unwrap_or_default();
                responses.push(
                    Element::new(Page::AirSync, "Add")
                        .with_text_child("ClientId", client_id)
                        .with_text_child("Status", status::CONVERSION_ERROR.to_string()),
                );
            }
            other => tracing::debug!(command = other, "ignoring unknown Sync command"),
        }
    }

    for (id, kind) in ctx.jmap.email_set(updates, destroy).await? {
        tracing::info!(device = %ctx.line.device_id, email = %id, error = %kind, "device change failed");
        touched.remove(&id);
        if let Some(sid) = change_sids.remove(&id) {
            let code = if kind == "notFound" {
                status::NOT_FOUND
            } else {
                status::SERVER_ERROR
            };
            responses.push(
                Element::new(Page::AirSync, "Change")
                    .with_text_child("ServerId", sid)
                    .with_text_child("Status", code.to_string()),
            );
        }
    }

    // New hashes for what the device changed, so it isn't echoed back.
    if !change_sids.is_empty() {
        let ids: Vec<String> = change_sids.keys().cloned().collect();
        let props = json!({ "properties": HASH_PROPERTIES });
        for e in ctx.jmap.email_get(&ids, &props).await? {
            let id = e.get("id").and_then(Value::as_str).unwrap_or_default();
            if let Some(sid) = change_sids.get(id) {
                let hash = email::state_hash(&e);
                if let Some(item) = items.get_mut(id) {
                    item.hash = hash.clone();
                }
                upsert.push(Item {
                    server_id: sid.clone(),
                    jmap_id: id.to_string(),
                    hash,
                });
            }
        }
    }

    if !fetch.is_empty() {
        let ids: Vec<String> = fetch.iter().map(|(_, id)| id.clone()).collect();
        let mut found: HashMap<String, Element> = serialise(ctx, &ids, body_opts)
            .await?
            .into_iter()
            .map(|(id, _, data)| (id, data))
            .collect();
        for (sid, id) in fetch {
            let el = Element::new(Page::AirSync, "Fetch").with_text_child("ServerId", sid);
            responses.push(match found.remove(&id) {
                Some(data) => el
                    .with_text_child("Status", status::SUCCESS.to_string())
                    .with_child(data),
                None => el.with_text_child("Status", status::NOT_FOUND.to_string()),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn el(page: Page, name: &str, text: &str) -> Element {
        Element::new(page, name).with_text(text)
    }

    #[test]
    fn parses_collections_and_options() {
        let req = Element::new(Page::AirSync, "Sync").with_child(
            Element::new(Page::AirSync, "Collections").with_child(
                Element::new(Page::AirSync, "Collection")
                    .with_child(el(Page::AirSync, "SyncKey", "1-abc"))
                    .with_child(el(Page::AirSync, "CollectionId", "2"))
                    .with_child(Element::new(Page::AirSync, "DeletesAsMoves"))
                    .with_child(el(Page::AirSync, "GetChanges", "0"))
                    .with_child(el(Page::AirSync, "WindowSize", "25"))
                    .with_child(
                        Element::new(Page::AirSync, "Options")
                            .with_child(el(Page::AirSync, "FilterType", "3"))
                            .with_child(
                                Element::new(Page::AirSyncBase, "BodyPreference")
                                    .with_child(el(Page::AirSyncBase, "Type", "2"))
                                    .with_child(el(Page::AirSyncBase, "TruncationSize", "5000")),
                            ),
                    )
                    .with_child(Element::new(Page::AirSync, "Commands").with_child(
                        Element::new(Page::AirSync, "Delete").with_child(el(
                            Page::AirSync,
                            "ServerId",
                            "2:x",
                        )),
                    )),
            ),
        );
        let p = parse_request(&req).unwrap();
        let c = &p.collections[0];
        assert_eq!(c.sync_key, "1-abc");
        assert_eq!(c.deletes_as_moves, Some(true));
        assert_eq!(c.get_changes, Some(false));
        assert_eq!(c.window_size, Some(25));
        assert_eq!(c.commands.len(), 1);
        let (filter, body) = parse_options(c.options.as_ref());
        assert_eq!(filter, 3);
        assert_eq!(body.preferences[0].body_type, BodyType::Html);
        assert_eq!(body.preferences[0].truncation_size, Some(5000));

        let mut stripped = req.clone();
        strip_commands(&mut stripped);
        assert!(parse_request(&stripped).unwrap().collections[0]
            .commands
            .is_empty());
    }

    #[test]
    fn wait_bounds() {
        let with = |name: &str, v: &str| {
            Element::new(Page::AirSync, "Sync").with_child(el(Page::AirSync, name, v))
        };
        assert_eq!(parse_request(&with("Wait", "5")).unwrap().wait, Some(300));
        assert_eq!(
            parse_request(&with("HeartbeatInterval", "600"))
                .unwrap()
                .wait,
            Some(600)
        );
        assert_eq!(
            parse_request(&with("Wait", "60")).unwrap_err(),
            status::INVALID_WAIT
        );
        assert_eq!(
            parse_request(&with("HeartbeatInterval", "10")).unwrap_err(),
            status::INVALID_WAIT
        );
    }

    #[test]
    fn filters() {
        assert!(filter_after(0).is_none());
        let week = filter_after(3).unwrap();
        assert!(week.ends_with('Z') && week.len() == 20);
    }
}
