//! `FolderSync`: the device's folder tree, built from JMAP mailboxes and,
//! when Stalwart offers them, address books.
//!
//! Folders get short numeric IDs that stay stable for the life of the
//! device record, even across a SyncKey 0 reset. The previous response is
//! kept so a retry with the previous key gets the same answer.

use super::{sync_key, wbxml, Ctx, HandlerError, HandlerResult};
use eas_jmap::mail::Mailbox;
use eas_state::{Folder, HierarchyCommit, HierarchyState};
use eas_wbxml::{Element, Page};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

/// FolderSync status codes (MS-ASCMD 2.2.3.177.4).
mod status {
    pub const SUCCESS: u8 = 1;
    pub const INVALID_SYNC_KEY: u8 = 9;
    pub const BAD_REQUEST: u8 = 10;
}

/// EAS folder types (MS-ASCMD 2.2.3.186.3).
pub mod folder_type {
    pub const INBOX: u8 = 2;
    pub const DRAFTS: u8 = 3;
    pub const TRASH: u8 = 4;
    pub const SENT: u8 = 5;
    pub const CALENDAR: u8 = 8;
    pub const CONTACTS: u8 = 9;
    pub const USER_MAIL: u8 = 12;
    pub const USER_CALENDAR: u8 = 13;
    pub const USER_CONTACTS: u8 = 14;
}

pub const KIND_MAILBOX: &str = "mailbox";
pub const KIND_ADDRESS_BOOK: &str = "addressbook";
pub const KIND_CALENDAR: &str = "calendar";

fn type_for_role(role: Option<&str>) -> u8 {
    match role {
        Some("inbox") => folder_type::INBOX,
        Some("drafts") => folder_type::DRAFTS,
        Some("trash") => folder_type::TRASH,
        Some("sent") => folder_type::SENT,
        _ => folder_type::USER_MAIL,
    }
}

/// The user-created type of a kind, used when a default type is taken.
fn user_type(kind: &str) -> u8 {
    match kind {
        KIND_ADDRESS_BOOK => folder_type::USER_CONTACTS,
        KIND_CALENDAR => folder_type::USER_CALENDAR,
        _ => folder_type::USER_MAIL,
    }
}

fn status_response(code: u8) -> Element {
    Element::new(Page::FolderHierarchy, "FolderSync").with_text_child("Status", code.to_string())
}

/// A JMAP collection that becomes an EAS folder.
#[derive(Debug, Clone)]
pub struct SourceFolder {
    pub kind: &'static str,
    pub id: String,
    pub name: String,
    pub parent_id: Option<String>,
    pub folder_type: u8,
    pub sort_order: u32,
}

pub fn from_mailboxes(mailboxes: &[Mailbox]) -> Vec<SourceFolder> {
    mailboxes
        .iter()
        .map(|m| SourceFolder {
            kind: KIND_MAILBOX,
            id: m.id.clone(),
            name: m.name.clone(),
            parent_id: m.parent_id.clone(),
            folder_type: type_for_role(m.role.as_deref()),
            sort_order: m.sort_order,
        })
        .collect()
}

/// Address books; the default one (or the first) is the device's Contacts
/// folder.
pub fn from_address_books(books: &[Value]) -> Vec<SourceFolder> {
    from_collections(books, KIND_ADDRESS_BOOK, "Contacts", folder_type::CONTACTS)
}

/// Calendars; the default one (or the first) is the device's Calendar.
pub fn from_calendars(calendars: &[Value]) -> Vec<SourceFolder> {
    from_collections(calendars, KIND_CALENDAR, "Calendar", folder_type::CALENDAR)
}

fn from_collections(
    books: &[Value],
    kind: &'static str,
    fallback_name: &str,
    default_type: u8,
) -> Vec<SourceFolder> {
    let default = books
        .iter()
        .position(|b| b.get("isDefault").and_then(Value::as_bool) == Some(true))
        .unwrap_or(0);
    books
        .iter()
        .enumerate()
        .filter_map(|(i, b)| {
            Some(SourceFolder {
                kind,
                id: b.get("id")?.as_str()?.to_string(),
                name: b
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or(fallback_name)
                    .to_string(),
                parent_id: None,
                folder_type: if i == default {
                    default_type
                } else {
                    user_type(kind)
                },
                sort_order: b.get("sortOrder").and_then(Value::as_u64).unwrap_or(0) as u32,
            })
        })
        .collect()
}

/// The folder list the device should have, parents before children, with
/// IDs taken from `known` or newly assigned from `next_id`.
pub fn desired_folders(
    sources: &[SourceFolder],
    known: &[Folder],
    next_id: &mut i64,
) -> Vec<Folder> {
    let mut ids: HashMap<(&str, &str), String> = known
        .iter()
        .map(|f| ((f.kind.as_str(), f.jmap_id.as_str()), f.server_id.clone()))
        .collect();
    let by_id: HashMap<(&str, &str), &SourceFolder> = sources
        .iter()
        .map(|m| ((m.kind, m.id.as_str()), m))
        .collect();
    let depth = |m: &SourceFolder| {
        let mut d = 0;
        let mut cur = m.parent_id.as_deref();
        while let Some(p) = cur.and_then(|p| by_id.get(&(m.kind, p))) {
            d += 1;
            if d > 64 {
                break;
            }
            cur = p.parent_id.as_deref();
        }
        d
    };
    let mut sorted: Vec<&SourceFolder> = sources.iter().collect();
    sorted.sort_by_key(|m| (m.kind, depth(m), m.sort_order, m.name.to_lowercase()));

    let mut used_defaults = HashSet::new();
    let mut out = Vec::with_capacity(sorted.len());
    for m in sorted {
        let server_id = ids
            .entry((m.kind, m.id.as_str()))
            .or_insert_with(|| {
                let id = next_id.to_string();
                *next_id += 1;
                id
            })
            .clone();
        let mut t = m.folder_type;
        // Each default folder type may appear once.
        if t != user_type(m.kind) && !used_defaults.insert(t) {
            t = user_type(m.kind);
        }
        let parent_id = m
            .parent_id
            .as_deref()
            .and_then(|p| ids.get(&(m.kind, p)))
            .cloned()
            .unwrap_or_else(|| "0".into());
        out.push(Folder {
            server_id,
            kind: m.kind.into(),
            jmap_id: m.id.clone(),
            folder_type: t,
            parent_id,
            display_name: m.name.clone(),
        });
    }
    out
}

fn folder_element(name: &str, f: &Folder) -> Element {
    Element::new(Page::FolderHierarchy, name)
        .with_text_child("ServerId", f.server_id.clone())
        .with_text_child("ParentId", f.parent_id.clone())
        .with_text_child("DisplayName", f.display_name.clone())
        .with_text_child("Type", f.folder_type.to_string())
}

pub async fn handle(ctx: &Ctx<'_>, request: Option<Element>) -> HandlerResult {
    let req = request.ok_or(HandlerError::BadRequest("empty FolderSync"))?;
    let Some(key) = req
        .child_text(Page::FolderHierarchy, "SyncKey")
        .map(|k| k.trim().to_string())
    else {
        return Ok(wbxml(&status_response(status::BAD_REQUEST)));
    };
    let store = &ctx.state.store;
    let dev = ctx.device.id;
    let _guard = ctx.state.locks.get(dev).lock_owned().await;

    let hs = store.hierarchy(dev).await?;
    let all_known = store.folders(dev).await?;
    let reset = key == "0";
    if !reset {
        match &hs {
            Some(h) if h.sync_key == key => {}
            Some(h) if h.prev_key.as_deref() == Some(key.as_str()) => {
                if let Some(prev) = h
                    .prev_response
                    .as_deref()
                    .and_then(|b| eas_wbxml::decode(b).ok())
                {
                    tracing::info!(device = %ctx.line.device_id, "FolderSync retry, replaying");
                    return Ok(wbxml(&prev));
                }
                return Ok(wbxml(&status_response(status::INVALID_SYNC_KEY)));
            }
            _ => return Ok(wbxml(&status_response(status::INVALID_SYNC_KEY))),
        }
    }

    let (mailboxes, _) = ctx.jmap.mailboxes().await?;
    let mut sources = from_mailboxes(&mailboxes);
    // Kinds listed this time; folders of other kinds are left alone.
    let mut managed = vec![KIND_MAILBOX];
    if let Some(account) = ctx.jmap.session().contacts() {
        let (books, _) = ctx
            .jmap
            .get_all(
                "AddressBook",
                account,
                Some(&["id", "name", "isDefault", "sortOrder"]),
            )
            .await?;
        sources.extend(from_address_books(&books));
        managed.push(KIND_ADDRESS_BOOK);
    }
    if let Some(account) = ctx.jmap.session().calendars() {
        let (cals, _) = ctx
            .jmap
            .get_all(
                "Calendar",
                account,
                Some(&["id", "name", "isDefault", "sortOrder"]),
            )
            .await?;
        sources.extend(from_calendars(&cals));
        managed.push(KIND_CALENDAR);
    }
    let mut next_id = hs.as_ref().map(|h| h.next_folder_id).unwrap_or(1);
    let desired = desired_folders(&sources, &all_known, &mut next_id);
    // On a reset the device knows nothing; IDs are still reused.
    let known: Vec<&Folder> = if reset {
        Vec::new()
    } else {
        all_known.iter().collect()
    };
    let known_by_id: HashMap<&str, &Folder> =
        known.iter().map(|f| (f.server_id.as_str(), *f)).collect();
    let desired_ids: HashSet<&str> = desired.iter().map(|f| f.server_id.as_str()).collect();

    let mut changes = Vec::new();
    for f in &known {
        if managed.contains(&f.kind.as_str()) && !desired_ids.contains(f.server_id.as_str()) {
            changes.push(
                Element::new(Page::FolderHierarchy, "Delete")
                    .with_text_child("ServerId", f.server_id.clone()),
            );
        }
    }
    let mut upsert = Vec::new();
    for f in &desired {
        match known_by_id.get(f.server_id.as_str()) {
            None => changes.push(folder_element("Add", f)),
            Some(old) if *old != f => changes.push(folder_element("Update", f)),
            Some(_) => continue,
        }
        upsert.push(f.clone());
    }
    // Mapping rows for mailboxes that are gone: announced deletes when the
    // device knew them, silent cleanup after a reset.
    let delete: Vec<String> = all_known
        .iter()
        .filter(|f| {
            managed.contains(&f.kind.as_str()) && !desired_ids.contains(f.server_id.as_str())
        })
        .map(|f| f.server_id.clone())
        .collect();

    if !reset && changes.is_empty() {
        let resp = status_response(status::SUCCESS)
            .with_text_child("SyncKey", key)
            .with_child(
                Element::new(Page::FolderHierarchy, "Changes").with_text_child("Count", "0"),
            );
        return Ok(wbxml(&resp));
    }

    let counter = hs.as_ref().map(|h| h.counter).unwrap_or(0) + 1;
    let new_key = sync_key(counter);
    let mut changes_el = Element::new(Page::FolderHierarchy, "Changes")
        .with_text_child("Count", changes.len().to_string());
    for c in changes {
        changes_el.push(c);
    }
    let resp = status_response(status::SUCCESS)
        .with_text_child("SyncKey", new_key.clone())
        .with_child(changes_el);
    store
        .commit_hierarchy(
            dev,
            HierarchyCommit {
                reset,
                state: Some(HierarchyState {
                    sync_key: new_key,
                    prev_key: (!reset).then_some(key),
                    prev_response: Some(eas_wbxml::encode(&resp)),
                    counter,
                    next_folder_id: next_id,
                }),
                upsert,
                delete,
            },
        )
        .await?;
    Ok(wbxml(&resp))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mb(id: &str, name: &str, parent: Option<&str>, role: Option<&str>) -> Mailbox {
        Mailbox {
            id: id.into(),
            name: name.into(),
            parent_id: parent.map(Into::into),
            role: role.map(Into::into),
            sort_order: 0,
        }
    }

    #[test]
    fn parents_first_and_ids_stable() {
        let boxes = vec![
            mb("c", "Child", Some("i"), None),
            mb("i", "Inbox", None, Some("inbox")),
            mb("t", "Trash", None, Some("trash")),
            mb("t2", "Trash 2", None, Some("trash")),
        ];
        let mut next = 1;
        let boxes = from_mailboxes(&boxes);
        let first = desired_folders(&boxes, &[], &mut next);
        assert_eq!(next, 5);
        let pos = |id: &str| first.iter().position(|f| f.jmap_id == id).unwrap();
        assert!(pos("i") < pos("c"));
        let child = &first[pos("c")];
        assert_eq!(child.parent_id, first[pos("i")].server_id);
        assert_eq!(first[pos("i")].folder_type, folder_type::INBOX);
        let trash_types: Vec<u8> = ["t", "t2"]
            .iter()
            .map(|id| first[pos(id)].folder_type)
            .collect();
        assert!(trash_types.contains(&folder_type::TRASH));
        assert!(trash_types.contains(&folder_type::USER_MAIL));

        // Same mailboxes again: same IDs, nothing new assigned.
        let again = desired_folders(&boxes, &first, &mut next);
        assert_eq!(next, 5);
        for f in &again {
            assert_eq!(
                f.server_id,
                first
                    .iter()
                    .find(|o| o.jmap_id == f.jmap_id)
                    .unwrap()
                    .server_id
            );
        }
    }
}
