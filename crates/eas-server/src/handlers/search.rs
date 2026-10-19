//! `Search` (MS-ASCMD 2.2.1.16) of the mailbox through `Email/query`.
//!
//! Results carry the JMAP email ID as `LongId`, which `ItemOperations` and
//! the smart commands accept. The GAL store answers with no results until
//! directory search lands with contacts.

use super::folder_sync::KIND_MAILBOX;
use super::sync::{parse_options, serialise};
use super::{wbxml, Ctx, HandlerError, HandlerResult};
use eas_wbxml::{Element, Page};
use serde_json::{json, Map, Value};
use std::collections::HashMap;

/// Search status codes (MS-ASCMD 2.2.3.177.12).
mod status {
    pub const SUCCESS: u8 = 1;
    pub const INVALID_REQUEST: u8 = 2;
    pub const SERVER_ERROR: u8 = 3;
    pub const FOLDER_SYNC_REQUIRED: u8 = 11;
}

/// Largest page of results per request.
const MAX_RANGE: usize = 100;

/// What the query asks for, gathered from `And` and its children.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Criteria {
    pub text: Option<String>,
    pub collections: Vec<String>,
    pub after: Option<String>,
    pub before: Option<String>,
}

fn date_value(cmp: &Element) -> Option<String> {
    let v = cmp.child_text(Page::Search, "Value")?;
    let dt = chrono::DateTime::parse_from_rfc3339(v.trim()).ok()?;
    Some(
        dt.with_timezone(&chrono::Utc)
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string(),
    )
}

fn collect(el: &Element, c: &mut Criteria, depth: usize) {
    if depth > 8 {
        return;
    }
    for child in el.elements() {
        match (child.page(), child.name()) {
            (Page::Search, "FreeText") => c.text = child.text().map(|t| t.trim().to_string()),
            (Page::AirSync, "CollectionId") => {
                if let Some(id) = child.text() {
                    c.collections.push(id.trim().to_string());
                }
            }
            (Page::Search, "GreaterThan") => c.after = date_value(child),
            (Page::Search, "LessThan") => c.before = date_value(child),
            (Page::Search, "And") | (Page::Search, "Or") => collect(child, c, depth + 1),
            _ => {}
        }
    }
}

pub fn criteria(query: &Element) -> Criteria {
    let mut c = Criteria::default();
    collect(query, &mut c, 0);
    c
}

/// `"0-99"` to (start, count).
pub fn parse_range(r: &str) -> Option<(usize, usize)> {
    let (a, b) = r.trim().split_once('-')?;
    let (a, b): (usize, usize) = (a.parse().ok()?, b.parse().ok()?);
    (b >= a).then(|| (a, (b - a + 1).min(MAX_RANGE)))
}

fn store_status(code: u8) -> Element {
    Element::new(Page::Search, "Store").with_text_child("Status", code.to_string())
}

fn wrap(store: Element) -> Element {
    Element::new(Page::Search, "Search")
        .with_text_child("Status", status::SUCCESS.to_string())
        .with_child(Element::new(Page::Search, "Response").with_child(store))
}

pub async fn handle(ctx: &Ctx<'_>, request: Option<Element>) -> HandlerResult {
    let req = request.ok_or(HandlerError::BadRequest("empty Search"))?;
    let Some(store_el) = req.child(Page::Search, "Store") else {
        return Ok(wbxml(
            &Element::new(Page::Search, "Search")
                .with_text_child("Status", status::INVALID_REQUEST.to_string()),
        ));
    };
    let name = store_el
        .child_text(Page::Search, "Name")
        .unwrap_or_default();
    if name.eq_ignore_ascii_case("GAL") {
        return Ok(wbxml(&wrap(
            store_status(status::SUCCESS)
                .with_child(Element::new(Page::Search, "Result"))
                .with_text_child("Total", "0"),
        )));
    }
    if !name.eq_ignore_ascii_case("Mailbox") {
        return Ok(wbxml(&wrap(store_status(status::INVALID_REQUEST))));
    }
    let Some(query) = store_el.child(Page::Search, "Query") else {
        return Ok(wbxml(&wrap(store_status(status::INVALID_REQUEST))));
    };
    let c = criteria(query);
    let options = store_el.child(Page::Search, "Options");
    let (start, count) = options
        .and_then(|o| o.child_text(Page::Search, "Range"))
        .and_then(|r| parse_range(&r))
        .unwrap_or((0, MAX_RANGE));
    let deep = options.is_some_and(|o| o.child(Page::Search, "DeepTraversal").is_some());

    let folders = ctx.state.store.folders(ctx.device.id).await?;
    let mut mailboxes = Vec::new();
    for id in &c.collections {
        match folders
            .iter()
            .find(|f| &f.server_id == id && f.kind == KIND_MAILBOX)
        {
            Some(f) => mailboxes.push(f.jmap_id.clone()),
            None => {
                return Ok(wbxml(&wrap(store_status(status::FOLDER_SYNC_REQUIRED))));
            }
        }
    }
    let mut conditions: Vec<Value> = Vec::new();
    if let Some(t) = c.text.as_ref().filter(|t| !t.is_empty()) {
        conditions.push(json!({ "text": t }));
    }
    if let Some(a) = &c.after {
        conditions.push(json!({ "after": a }));
    }
    if let Some(b) = &c.before {
        conditions.push(json!({ "before": b }));
    }
    // DeepTraversal would include subfolders; JMAP's inMailbox doesn't, so
    // a deep search of one folder searches its subtree explicitly.
    if !mailboxes.is_empty() {
        let mut ids = mailboxes.clone();
        if deep {
            let parent_of: HashMap<&str, &str> = folders
                .iter()
                .map(|f| (f.server_id.as_str(), f.parent_id.as_str()))
                .collect();
            for f in folders.iter().filter(|f| f.kind == KIND_MAILBOX) {
                let mut cur = f.parent_id.as_str();
                let mut hops = 0;
                while cur != "0" && hops < 64 {
                    if c.collections.iter().any(|x| x == cur) {
                        ids.push(f.jmap_id.clone());
                        break;
                    }
                    cur = parent_of.get(cur).copied().unwrap_or("0");
                    hops += 1;
                }
            }
        }
        let ors: Vec<Value> = ids.iter().map(|m| json!({ "inMailbox": m })).collect();
        conditions.push(json!({ "operator": "OR", "conditions": ors }));
    }
    if conditions.is_empty() {
        return Ok(wbxml(&wrap(store_status(status::INVALID_REQUEST))));
    }
    let filter = json!({ "operator": "AND", "conditions": conditions });

    let (ids, total) = match ctx.jmap.email_search(filter, start, count).await {
        Ok(r) => r,
        Err(e @ (eas_jmap::JmapError::Unauthorized | eas_jmap::JmapError::Throttled)) => {
            return Err(e.into())
        }
        Err(e) => {
            tracing::warn!(error = %e, "search failed");
            return Ok(wbxml(&wrap(store_status(status::SERVER_ERROR))));
        }
    };
    let (_, body_opts) = parse_options(options);
    let by_jmap: HashMap<&str, &str> = folders
        .iter()
        .filter(|f| f.kind == KIND_MAILBOX)
        .map(|f| (f.jmap_id.as_str(), f.server_id.as_str()))
        .collect();

    let mut store = store_status(status::SUCCESS);
    let found = serialise(ctx, &ids, &body_opts).await?;
    if found.is_empty() {
        store.push(Element::new(Page::Search, "Result"));
    }
    let n = found.len();
    for (id, email, data) in found {
        let folder = email
            .get("mailboxIds")
            .and_then(Value::as_object)
            .and_then(|m: &Map<String, Value>| m.keys().find_map(|k| by_jmap.get(k.as_str())))
            .copied()
            .unwrap_or_default();
        let mut props = Element::new(Page::Search, "Properties");
        props.children = data.children;
        store.push(
            Element::new(Page::Search, "Result")
                .with_child(Element::new(Page::AirSync, "Class").with_text("Email"))
                .with_text_child("LongId", id)
                .with_child(Element::new(Page::AirSync, "CollectionId").with_text(folder))
                .with_child(props),
        );
    }
    if n > 0 {
        store = store.with_text_child("Range", format!("{start}-{}", start + n - 1));
    }
    store = store.with_text_child("Total", total.to_string());
    Ok(wbxml(&wrap(store)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn criteria_and_ranges() {
        let q = Element::new(Page::Search, "Query").with_child(
            Element::new(Page::Search, "And")
                .with_child(Element::new(Page::AirSync, "Class").with_text("Email"))
                .with_child(Element::new(Page::AirSync, "CollectionId").with_text("2"))
                .with_child(Element::new(Page::Search, "FreeText").with_text(" invoice "))
                .with_child(
                    Element::new(Page::Search, "GreaterThan")
                        .with_child(Element::new(Page::Email, "DateReceived"))
                        .with_child(
                            Element::new(Page::Search, "Value")
                                .with_text("2026-10-01T00:00:00.000Z"),
                        ),
                ),
        );
        let c = criteria(&q);
        assert_eq!(c.text.as_deref(), Some("invoice"));
        assert_eq!(c.collections, vec!["2"]);
        assert_eq!(c.after.as_deref(), Some("2026-10-01T00:00:00Z"));
        assert_eq!(parse_range("0-9"), Some((0, 10)));
        assert_eq!(parse_range("10-500"), Some((10, MAX_RANGE)));
        assert_eq!(parse_range("9-1"), None);
    }
}
