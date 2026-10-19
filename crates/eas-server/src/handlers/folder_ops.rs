//! `FolderCreate`, `FolderUpdate` and `FolderDelete` (MS-ASCMD 2.2.1.2-4)
//! for mail folders.
//!
//! Each one needs the current hierarchy SyncKey, changes the mailbox
//! through `Mailbox/set`, records the change in the device's folder map so
//! the next `FolderSync` doesn't announce it again, and answers with a new
//! hierarchy key.

use super::folder_sync::{folder_type, KIND_MAILBOX};
use super::{sync_key, wbxml, Ctx, HandlerError, HandlerResult};
use eas_jmap::{SetOp, SetOutcome};
use eas_proto::Command;
use eas_state::{Folder, HierarchyCommit, HierarchyState};
use eas_wbxml::{Element, Page};
use serde_json::{json, Value};

/// Folder command status codes (MS-ASCMD 2.2.3.177.2-4).
mod status {
    pub const SUCCESS: u8 = 1;
    pub const ALREADY_EXISTS: u8 = 2;
    pub const SPECIAL_FOLDER: u8 = 3;
    pub const NOT_FOUND: u8 = 4;
    pub const PARENT_NOT_FOUND: u8 = 5;
    pub const SERVER_ERROR: u8 = 6;
    pub const INVALID_SYNC_KEY: u8 = 9;
    pub const BAD_REQUEST: u8 = 10;
}

fn reply(root: &str, code: u8) -> Element {
    Element::new(Page::FolderHierarchy, root).with_text_child("Status", code.to_string())
}

fn set_status(kind: &str) -> u8 {
    match kind {
        "alreadyExists" => status::ALREADY_EXISTS,
        "notFound" => status::NOT_FOUND,
        "forbidden" => status::SPECIAL_FOLDER,
        _ => status::SERVER_ERROR,
    }
}

pub async fn handle(ctx: &Ctx<'_>, command: Command, request: Option<Element>) -> HandlerResult {
    let root = command.name();
    let req = request.ok_or(HandlerError::BadRequest("empty folder request"))?;
    if !req.is(Page::FolderHierarchy, root) {
        return Ok(wbxml(&reply(root, status::BAD_REQUEST)));
    }
    let text = |name: &str| {
        req.child_text(Page::FolderHierarchy, name)
            .map(|t| t.trim().to_string())
    };
    let Some(key) = text("SyncKey") else {
        return Ok(wbxml(&reply(root, status::BAD_REQUEST)));
    };
    let store = &ctx.state.store;
    let dev = ctx.device.id;
    let _guard = ctx.state.locks.get(dev).lock_owned().await;
    let Some(hs) = store.hierarchy(dev).await? else {
        return Ok(wbxml(&reply(root, status::INVALID_SYNC_KEY)));
    };
    if hs.sync_key != key {
        return Ok(wbxml(&reply(root, status::INVALID_SYNC_KEY)));
    }
    let folders = store.folders(dev).await?;
    let by_id = |id: &str| folders.iter().find(|f| f.server_id == id);
    // Resolves a parent ID to a JMAP mailbox (`None` for the top level).
    let parent_of = |id: &str| -> Result<Option<&Folder>, u8> {
        if id == "0" {
            return Ok(None);
        }
        match by_id(id) {
            Some(f) if f.kind == KIND_MAILBOX => Ok(Some(f)),
            _ => Err(status::PARENT_NOT_FOUND),
        }
    };
    let account = ctx.jmap.account().to_string();
    let mut next_folder_id = hs.next_folder_id;
    let mut commit = HierarchyCommit::default();
    let mut new_server_id = None;

    match command {
        Command::FolderCreate => {
            let (Some(name), Some(parent), Some(t)) =
                (text("DisplayName"), text("ParentId"), text("Type"))
            else {
                return Ok(wbxml(&reply(root, status::BAD_REQUEST)));
            };
            // Only mail folders for now; calendars and contacts come with
            // their data classes.
            if !matches!(t.as_str(), "1" | "12") || name.is_empty() {
                return Ok(wbxml(&reply(root, status::BAD_REQUEST)));
            }
            let parent = match parent_of(&parent) {
                Ok(p) => p,
                Err(code) => return Ok(wbxml(&reply(root, code))),
            };
            let siblings_clash = folders.iter().any(|f| {
                f.kind == KIND_MAILBOX
                    && f.parent_id == parent.map(|p| p.server_id.as_str()).unwrap_or("0")
                    && f.display_name.eq_ignore_ascii_case(&name)
            });
            if siblings_clash {
                return Ok(wbxml(&reply(root, status::ALREADY_EXISTS)));
            }
            let obj = json!({
                "name": name,
                "parentId": parent.map(|p| p.jmap_id.clone()),
            });
            match ctx
                .jmap
                .set_one("Mailbox/set", &account, SetOp::Create(obj), Value::Null)
                .await?
            {
                SetOutcome::Created(id) => {
                    let server_id = next_folder_id.to_string();
                    next_folder_id += 1;
                    commit.upsert.push(Folder {
                        server_id: server_id.clone(),
                        kind: KIND_MAILBOX.into(),
                        jmap_id: id,
                        folder_type: folder_type::USER_MAIL,
                        parent_id: parent
                            .map(|p| p.server_id.clone())
                            .unwrap_or_else(|| "0".into()),
                        display_name: name,
                    });
                    new_server_id = Some(server_id);
                }
                SetOutcome::Failed(kind) => return Ok(wbxml(&reply(root, set_status(&kind)))),
                _ => return Ok(wbxml(&reply(root, status::SERVER_ERROR))),
            }
        }
        Command::FolderUpdate => {
            let (Some(id), Some(name), Some(parent)) =
                (text("ServerId"), text("DisplayName"), text("ParentId"))
            else {
                return Ok(wbxml(&reply(root, status::BAD_REQUEST)));
            };
            let Some(folder) = by_id(&id).filter(|f| f.kind == KIND_MAILBOX) else {
                return Ok(wbxml(&reply(root, status::NOT_FOUND)));
            };
            if folder.folder_type != folder_type::USER_MAIL {
                return Ok(wbxml(&reply(root, status::SPECIAL_FOLDER)));
            }
            let parent = match parent_of(&parent) {
                Ok(p) => p,
                Err(code) => return Ok(wbxml(&reply(root, code))),
            };
            let patch = json!({
                "name": name,
                "parentId": parent.map(|p| p.jmap_id.clone()),
            });
            match ctx
                .jmap
                .set_one(
                    "Mailbox/set",
                    &account,
                    SetOp::Update(folder.jmap_id.clone(), patch),
                    Value::Null,
                )
                .await?
            {
                SetOutcome::Updated => {
                    let mut f = folder.clone();
                    f.display_name = name;
                    f.parent_id = parent
                        .map(|p| p.server_id.clone())
                        .unwrap_or_else(|| "0".into());
                    commit.upsert.push(f);
                }
                SetOutcome::Failed(kind) => return Ok(wbxml(&reply(root, set_status(&kind)))),
                _ => return Ok(wbxml(&reply(root, status::SERVER_ERROR))),
            }
        }
        Command::FolderDelete => {
            let Some(id) = text("ServerId") else {
                return Ok(wbxml(&reply(root, status::BAD_REQUEST)));
            };
            let Some(folder) = by_id(&id).filter(|f| f.kind == KIND_MAILBOX) else {
                return Ok(wbxml(&reply(root, status::NOT_FOUND)));
            };
            if folder.folder_type != folder_type::USER_MAIL {
                return Ok(wbxml(&reply(root, status::SPECIAL_FOLDER)));
            }
            // The device drops the folder's subfolders with it, so they go
            // too; JMAP refuses to destroy a mailbox that has children.
            let mut doomed = vec![folder.clone()];
            let mut i = 0;
            while i < doomed.len() {
                let parent = doomed[i].server_id.clone();
                doomed.extend(folders.iter().filter(|f| f.parent_id == parent).cloned());
                i += 1;
            }
            for f in doomed.iter().rev() {
                match ctx
                    .jmap
                    .set_one(
                        "Mailbox/set",
                        &account,
                        SetOp::Destroy(f.jmap_id.clone()),
                        json!({ "onDestroyRemoveEmails": true }),
                    )
                    .await?
                {
                    SetOutcome::Destroyed => commit.delete.push(f.server_id.clone()),
                    SetOutcome::Failed(kind) if kind == "notFound" => {
                        commit.delete.push(f.server_id.clone())
                    }
                    SetOutcome::Failed(kind) => {
                        // Keep the map in step with what was destroyed.
                        tracing::warn!(folder = %f.server_id, %kind, "FolderDelete failed");
                        if !commit.delete.is_empty() {
                            store.commit_hierarchy(dev, commit).await?;
                        }
                        return Ok(wbxml(&reply(root, set_status(&kind))));
                    }
                    _ => return Ok(wbxml(&reply(root, status::SERVER_ERROR))),
                }
            }
        }
        _ => return Err(HandlerError::BadRequest("not a folder command")),
    }

    let counter = hs.counter + 1;
    let new_key = sync_key(counter);
    commit.state = Some(HierarchyState {
        sync_key: new_key.clone(),
        prev_key: Some(key),
        prev_response: None,
        counter,
        next_folder_id,
    });
    store.commit_hierarchy(dev, commit).await?;
    let mut resp = reply(root, status::SUCCESS).with_text_child("SyncKey", new_key);
    if let Some(id) = new_server_id {
        resp = resp.with_text_child("ServerId", id);
    }
    Ok(wbxml(&resp))
}
