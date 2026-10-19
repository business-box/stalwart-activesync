//! Calls that work the same for every JMAP data type: `/get`, `/changes`
//! and `/query` (RFC 8620, section 5). The mail helpers and the contacts
//! and calendar code are built on these.

use crate::client::{Call, JmapClient};
use crate::mail::Changes;
use crate::session::JmapError;
use serde_json::{json, Map, Value};

const GET_CHUNK: usize = 100;
const QUERY_PAGE: usize = 1_000;
const CHANGES_MAX_PAGES: usize = 20;

pub(crate) fn str_list(v: &Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn state_of(v: &Value, key: &str) -> Result<String, JmapError> {
    v.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| JmapError::Protocol(format!("missing {key}")))
}

impl JmapClient {
    /// All objects of `kind` (`ids: null`) with the current state; for
    /// small collections such as mailboxes or address books.
    pub async fn get_all(
        &self,
        kind: &str,
        account: &str,
        properties: Option<&[&str]>,
    ) -> Result<(Vec<Value>, String), JmapError> {
        let res = self
            .call(
                &format!("{kind}/get"),
                json!({ "accountId": account, "ids": null, "properties": properties }),
            )
            .await?;
        let list = res
            .get("list")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        Ok((list, state_of(&res, "state")?))
    }

    /// The current state of `kind`, without fetching anything.
    pub async fn state(&self, kind: &str, account: &str) -> Result<String, JmapError> {
        let res = self
            .call(
                &format!("{kind}/get"),
                json!({ "accountId": account, "ids": [], "properties": ["id"] }),
            )
            .await?;
        state_of(&res, "state")
    }

    /// `/get` for `ids` with extra arguments, in chunks; missing IDs are
    /// skipped.
    pub async fn get(
        &self,
        kind: &str,
        account: &str,
        ids: &[String],
        args: &Value,
    ) -> Result<Vec<Value>, JmapError> {
        let method = format!("{kind}/get");
        let calls: Vec<Call> = ids
            .chunks(GET_CHUNK)
            .enumerate()
            .map(|(i, chunk)| {
                let mut a = args.as_object().cloned().unwrap_or_default();
                a.insert("accountId".into(), json!(account));
                a.insert("ids".into(), json!(chunk));
                Call::new(&method, Value::Object(a), format!("g{i}"))
            })
            .collect();
        if calls.is_empty() {
            return Ok(Vec::new());
        }
        let mut out = Vec::with_capacity(ids.len());
        for res in self.batch(calls).await? {
            if let Some(list) = res.get("list").and_then(Value::as_array) {
                out.extend(list.iter().cloned());
            }
        }
        Ok(out)
    }

    /// `/changes` since `since`, following `hasMoreChanges`. Fails with
    /// `cannotCalculateChanges` when the state is too old.
    pub async fn changes(
        &self,
        kind: &str,
        account: &str,
        since: &str,
    ) -> Result<Changes, JmapError> {
        let method = format!("{kind}/changes");
        let mut out = Changes::default();
        let mut since = since.to_string();
        for _ in 0..CHANGES_MAX_PAGES {
            let res = self
                .call(
                    &method,
                    json!({ "accountId": account, "sinceState": since, "maxChanges": 5_000 }),
                )
                .await?;
            out.created.extend(str_list(&res, "created"));
            out.updated.extend(str_list(&res, "updated"));
            out.destroyed.extend(str_list(&res, "destroyed"));
            out.new_state = state_of(&res, "newState")?;
            if res.get("hasMoreChanges").and_then(Value::as_bool) != Some(true) {
                return Ok(out);
            }
            since = out.new_state.clone();
        }
        Err(JmapError::Method {
            method,
            kind: "cannotCalculateChanges".into(),
            description: Some("too many changes".into()),
        })
    }

    /// Every ID matching `filter`, in pages, up to `max`. `extra` adds
    /// arguments such as `sort` or `collapseThreads`.
    pub async fn query_ids(
        &self,
        kind: &str,
        account: &str,
        filter: Value,
        extra: Map<String, Value>,
        max: usize,
    ) -> Result<Vec<String>, JmapError> {
        let method = format!("{kind}/query");
        let mut ids = Vec::new();
        loop {
            let mut args = extra.clone();
            args.insert("accountId".into(), json!(account));
            args.insert("filter".into(), filter.clone());
            args.insert("position".into(), json!(ids.len()));
            args.insert("limit".into(), json!(QUERY_PAGE));
            let res = self.call(&method, Value::Object(args)).await?;
            let page = str_list(&res, "ids");
            let n = page.len();
            ids.extend(page);
            if n < QUERY_PAGE || ids.len() >= max {
                break;
            }
        }
        ids.truncate(max);
        Ok(ids)
    }
}
