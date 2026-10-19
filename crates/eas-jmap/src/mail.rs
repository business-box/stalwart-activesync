//! Typed helpers for the JMAP mail calls the sync engine needs (RFC 8621).

use crate::client::{Call, JmapClient};
use crate::session::JmapError;
use serde::Deserialize;
use serde_json::{json, Map, Value};

/// A mailbox as `FolderSync` sees it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mailbox {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub sort_order: u32,
}

/// Result of `Email/changes`, collected across `hasMoreChanges` pages.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Changes {
    pub new_state: String,
    pub created: Vec<String>,
    pub updated: Vec<String>,
    pub destroyed: Vec<String>,
}

/// IDs per `Email/get`; Stalwart's default `maxObjectsInGet` is 500.
const GET_CHUNK: usize = 100;
/// Page size for `Email/query`.
const QUERY_PAGE: usize = 1_000;
/// Hard stop for one mailbox listing, so a huge folder with no time filter
/// can't make a single sync unbounded.
pub const QUERY_MAX: usize = 50_000;
/// Pages followed in `Email/changes` before giving up and resyncing flags.
const CHANGES_MAX_PAGES: usize = 20;

fn str_list(v: &Value, key: &str) -> Vec<String> {
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

fn state(v: &Value, key: &str) -> Result<String, JmapError> {
    v.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| JmapError::Protocol(format!("missing {key}")))
}

impl JmapClient {
    /// All mailboxes and the current `Mailbox` state.
    pub async fn mailboxes(&self) -> Result<(Vec<Mailbox>, String), JmapError> {
        let res = self
            .call(
                "Mailbox/get",
                json!({
                    "accountId": self.account(),
                    "ids": null,
                    "properties": ["id", "name", "parentId", "role", "sortOrder"],
                }),
            )
            .await?;
        let list = serde_json::from_value(res.get("list").cloned().unwrap_or(Value::Null))
            .map_err(|e| JmapError::Protocol(format!("Mailbox/get: {e}")))?;
        Ok((list, state(&res, "state")?))
    }

    /// The current `Email` state, without fetching anything.
    pub async fn email_state(&self) -> Result<String, JmapError> {
        let res = self
            .call(
                "Email/get",
                json!({ "accountId": self.account(), "ids": [], "properties": ["id"] }),
            )
            .await?;
        state(&res, "state")
    }

    /// IDs of the emails in `mailbox`, newest first, optionally only those
    /// received after `after` (UTC date, RFC 3339).
    pub async fn email_ids(
        &self,
        mailbox: &str,
        after: Option<&str>,
    ) -> Result<Vec<String>, JmapError> {
        let mut filter = Map::new();
        filter.insert("inMailbox".into(), json!(mailbox));
        if let Some(after) = after {
            filter.insert("after".into(), json!(after));
        }
        let mut ids = Vec::new();
        loop {
            let res = self
                .call(
                    "Email/query",
                    json!({
                        "accountId": self.account(),
                        "filter": filter,
                        "sort": [{"property": "receivedAt", "isAscending": false}],
                        "collapseThreads": false,
                        "position": ids.len(),
                        "limit": QUERY_PAGE,
                    }),
                )
                .await?;
            let page = str_list(&res, "ids");
            let n = page.len();
            ids.extend(page);
            if n < QUERY_PAGE || ids.len() >= QUERY_MAX {
                break;
            }
        }
        ids.truncate(QUERY_MAX);
        Ok(ids)
    }

    /// `Email/changes` since `since`, following `hasMoreChanges`. Fails with
    /// `cannotCalculateChanges` when the state is too old; callers then fall
    /// back to a full comparison.
    pub async fn email_changes(&self, since: &str) -> Result<Changes, JmapError> {
        let mut out = Changes::default();
        let mut since = since.to_string();
        for _ in 0..CHANGES_MAX_PAGES {
            let res = self
                .call(
                    "Email/changes",
                    json!({ "accountId": self.account(), "sinceState": since, "maxChanges": 5_000 }),
                )
                .await?;
            out.created.extend(str_list(&res, "created"));
            out.updated.extend(str_list(&res, "updated"));
            out.destroyed.extend(str_list(&res, "destroyed"));
            out.new_state = state(&res, "newState")?;
            if res.get("hasMoreChanges").and_then(Value::as_bool) != Some(true) {
                return Ok(out);
            }
            since = out.new_state.clone();
        }
        Err(JmapError::Method {
            method: "Email/changes".into(),
            kind: "cannotCalculateChanges".into(),
            description: Some("too many changes".into()),
        })
    }

    /// `Email/get` for `ids` with extra arguments (`properties`,
    /// `fetchHTMLBodyValues`, ...), in chunks. Missing IDs are skipped.
    pub async fn email_get(&self, ids: &[String], args: &Value) -> Result<Vec<Value>, JmapError> {
        let calls = ids
            .chunks(GET_CHUNK)
            .enumerate()
            .map(|(i, chunk)| {
                let mut a = args.as_object().cloned().unwrap_or_default();
                a.insert("accountId".into(), json!(self.account()));
                a.insert("ids".into(), json!(chunk));
                Call::new("Email/get", Value::Object(a), format!("g{i}"))
            })
            .collect::<Vec<_>>();
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

    /// `Email/set` with `update` patches keyed by email ID and `destroy` IDs.
    /// Returns the IDs that failed with their JMAP error type.
    pub async fn email_set(
        &self,
        update: Map<String, Value>,
        destroy: Vec<String>,
    ) -> Result<Vec<(String, String)>, JmapError> {
        if update.is_empty() && destroy.is_empty() {
            return Ok(Vec::new());
        }
        let res = self
            .call(
                "Email/set",
                json!({ "accountId": self.account(), "update": update, "destroy": destroy }),
            )
            .await?;
        let mut failed = Vec::new();
        for key in ["notUpdated", "notDestroyed"] {
            if let Some(map) = res.get(key).and_then(Value::as_object) {
                for (id, err) in map {
                    let kind = err
                        .get("type")
                        .and_then(Value::as_str)
                        .unwrap_or("serverFail");
                    failed.push((id.clone(), kind.to_string()));
                }
            }
        }
        Ok(failed)
    }
}

/// A sending identity.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Identity {
    pub id: String,
    #[serde(default)]
    pub email: String,
}

/// Picks the identity for a sender address: an exact match, then a
/// `*@domain` wildcard, then the first one.
pub fn pick_identity<'a>(identities: &'a [Identity], from: &str) -> Option<&'a Identity> {
    let from = from.to_ascii_lowercase();
    let domain = from.rsplit_once('@').map(|(_, d)| d.to_string());
    identities
        .iter()
        .find(|i| i.email.to_ascii_lowercase() == from)
        .or_else(|| {
            identities.iter().find(|i| {
                domain
                    .as_ref()
                    .is_some_and(|d| i.email.to_ascii_lowercase() == format!("*@{d}"))
            })
        })
        .or_else(|| identities.first())
}

impl JmapClient {
    pub async fn identities(&self) -> Result<Vec<Identity>, JmapError> {
        let res = self
            .call(
                "Identity/get",
                json!({ "accountId": self.account(), "ids": null, "properties": ["id", "email"] }),
            )
            .await?;
        serde_json::from_value(res.get("list").cloned().unwrap_or(Value::Null))
            .map_err(|e| JmapError::Protocol(format!("Identity/get: {e}")))
    }

    /// Stores `mime` in `mailbox` and submits it with an explicit envelope.
    /// With `keep == false` the stored copy is destroyed once submitted.
    /// Returns the email's ID.
    #[allow(clippy::too_many_arguments)]
    pub async fn submit(
        &self,
        mime: Vec<u8>,
        identity_id: &str,
        mail_from: &str,
        rcpt_to: &[String],
        mailbox: &str,
        keep: bool,
    ) -> Result<String, JmapError> {
        let blob = self.upload(mime, "message/rfc822").await?;
        let mut submission = json!({
            "accountId": self.account(),
            "create": {
                "s": {
                    "identityId": identity_id,
                    "emailId": "#m",
                    "envelope": {
                        "mailFrom": {"email": mail_from},
                        "rcptTo": rcpt_to.iter().map(|r| json!({"email": r})).collect::<Vec<_>>(),
                    },
                }
            },
        });
        if !keep {
            submission["onSuccessDestroyEmail"] = json!(["#s"]);
        }
        let res = self
            .batch(vec![
                Call::new(
                    "Email/import",
                    json!({
                        "accountId": self.account(),
                        "emails": {"m": {
                            "blobId": blob,
                            "mailboxIds": {mailbox: true},
                            "keywords": {"$seen": true},
                        }},
                    }),
                    "i",
                ),
                Call::new("EmailSubmission/set", submission, "s"),
            ])
            .await?;
        let not_created = |v: &Value, key: &str, method: &str| -> Result<(), JmapError> {
            match v.get("notCreated").and_then(|n| n.get(key)) {
                Some(err) => Err(JmapError::Method {
                    method: method.into(),
                    kind: err
                        .get("type")
                        .and_then(Value::as_str)
                        .unwrap_or("serverFail")
                        .to_string(),
                    description: err
                        .get("description")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                }),
                None => Ok(()),
            }
        };
        not_created(&res[0], "m", "Email/import")?;
        not_created(&res[1], "s", "EmailSubmission/set")?;
        res[0]
            .get("created")
            .and_then(|c| c.get("m"))
            .and_then(|m| m.get("id"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| JmapError::Protocol("Email/import without created id".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mailbox_deserialises_with_defaults() {
        let m: Mailbox =
            serde_json::from_value(json!({"id": "a", "name": "Inbox", "role": "inbox"})).unwrap();
        assert_eq!(m.parent_id, None);
        assert_eq!(m.role.as_deref(), Some("inbox"));
        assert_eq!(m.sort_order, 0);
    }

    #[test]
    fn identity_choice() {
        let ids = vec![
            Identity {
                id: "1".into(),
                email: "info@example.org".into(),
            },
            Identity {
                id: "2".into(),
                email: "*@example.org".into(),
            },
            Identity {
                id: "3".into(),
                email: "Bob@Example.org".into(),
            },
        ];
        assert_eq!(pick_identity(&ids, "bob@example.org").unwrap().id, "3");
        assert_eq!(pick_identity(&ids, "sales@example.org").unwrap().id, "2");
        assert_eq!(pick_identity(&ids, "x@other.org").unwrap().id, "1");
        assert!(pick_identity(&[], "x@other.org").is_none());
    }
}
