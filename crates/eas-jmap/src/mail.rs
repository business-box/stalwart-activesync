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

/// Hard stop for one mailbox listing, so a huge folder with no time filter
/// can't make a single sync unbounded.
pub const QUERY_MAX: usize = 50_000;

use crate::objects::{state_of as state, str_list};

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
        self.state("Email", self.account()).await
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
        let mut extra = Map::new();
        extra.insert(
            "sort".into(),
            json!([{"property": "receivedAt", "isAscending": false}]),
        );
        extra.insert("collapseThreads".into(), json!(false));
        self.query_ids(
            "Email",
            self.account(),
            Value::Object(filter),
            extra,
            QUERY_MAX,
        )
        .await
    }

    /// `Email/changes` since `since`, following `hasMoreChanges`.
    pub async fn email_changes(&self, since: &str) -> Result<Changes, JmapError> {
        self.changes("Email", self.account(), since).await
    }

    /// `Email/get` for `ids` with extra arguments, in chunks.
    pub async fn email_get(&self, ids: &[String], args: &Value) -> Result<Vec<Value>, JmapError> {
        self.get("Email", self.account(), ids, args).await
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
    pub name: String,
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
                json!({ "accountId": self.account(), "ids": null, "properties": ["id", "name", "email"] }),
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

/// The account's vacation response (RFC 8621, section 8).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Vacation {
    #[serde(default)]
    pub is_enabled: bool,
    #[serde(default)]
    pub from_date: Option<String>,
    #[serde(default)]
    pub to_date: Option<String>,
    #[serde(default)]
    pub text_body: Option<String>,
    #[serde(default)]
    pub html_body: Option<String>,
}

impl JmapClient {
    pub async fn vacation(&self) -> Result<Vacation, JmapError> {
        let res = self
            .call(
                "VacationResponse/get",
                json!({ "accountId": self.account(), "ids": ["singleton"] }),
            )
            .await?;
        let first = res
            .get("list")
            .and_then(Value::as_array)
            .and_then(|l| l.first())
            .cloned()
            .unwrap_or(Value::Null);
        if first.is_null() {
            return Ok(Vacation::default());
        }
        serde_json::from_value(first)
            .map_err(|e| JmapError::Protocol(format!("VacationResponse/get: {e}")))
    }

    pub async fn set_vacation(&self, v: &Vacation) -> Result<(), JmapError> {
        let res = self
            .call(
                "VacationResponse/set",
                json!({
                    "accountId": self.account(),
                    "update": {"singleton": {
                        "isEnabled": v.is_enabled,
                        "fromDate": v.from_date,
                        "toDate": v.to_date,
                        "textBody": v.text_body,
                        "htmlBody": v.html_body,
                    }},
                }),
            )
            .await?;
        if let Some(err) = res.get("notUpdated").and_then(|n| n.get("singleton")) {
            return Err(JmapError::Method {
                method: "VacationResponse/set".into(),
                kind: err
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("serverFail")
                    .to_string(),
                description: None,
            });
        }
        Ok(())
    }

    /// One page of a search: matching email IDs, newest first, and the
    /// total number of matches.
    pub async fn email_search(
        &self,
        filter: Value,
        position: usize,
        limit: usize,
    ) -> Result<(Vec<String>, usize), JmapError> {
        let res = self
            .call(
                "Email/query",
                json!({
                    "accountId": self.account(),
                    "filter": filter,
                    "sort": [{"property": "receivedAt", "isAscending": false}],
                    "collapseThreads": false,
                    "position": position,
                    "limit": limit,
                    "calculateTotal": true,
                }),
            )
            .await?;
        let total = res.get("total").and_then(Value::as_u64).unwrap_or(0) as usize;
        Ok((str_list(&res, "ids"), total))
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
                name: String::new(),
                email: "info@example.org".into(),
            },
            Identity {
                id: "2".into(),
                name: String::new(),
                email: "*@example.org".into(),
            },
            Identity {
                id: "3".into(),
                name: String::new(),
                email: "Bob@Example.org".into(),
            },
        ];
        assert_eq!(pick_identity(&ids, "bob@example.org").unwrap().id, "3");
        assert_eq!(pick_identity(&ids, "sales@example.org").unwrap().id, "2");
        assert_eq!(pick_identity(&ids, "x@other.org").unwrap().id, "1");
        assert!(pick_identity(&[], "x@other.org").is_none());
    }
}
