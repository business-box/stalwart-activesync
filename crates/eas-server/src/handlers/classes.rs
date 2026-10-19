//! The data classes the Sync engine handles and what differs between them:
//! which JMAP type backs a folder, how items are listed, fingerprinted and
//! serialised.

use super::folder_sync::{KIND_ADDRESS_BOOK, KIND_MAILBOX};
use super::{Ctx, HandlerError};
use eas_convert::{contact, email};
use eas_jmap::JmapError;
use eas_state::Folder;
use eas_wbxml::Element;
use serde_json::{json, Map, Value};

/// Most items listed for one contacts folder.
const MAX_CONTACTS: usize = 50_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Mail,
    Contacts,
}

impl Class {
    pub fn of(folder: &Folder) -> Option<Class> {
        match folder.kind.as_str() {
            KIND_MAILBOX => Some(Class::Mail),
            KIND_ADDRESS_BOOK => Some(Class::Contacts),
            _ => None,
        }
    }

    /// The JMAP type of the items.
    pub fn jmap_type(self) -> &'static str {
        match self {
            Class::Mail => "Email",
            Class::Contacts => "ContactCard",
        }
    }

    pub fn account(self, ctx: &Ctx<'_>) -> Result<String, HandlerError> {
        let session = ctx.jmap.session();
        let account = match self {
            Class::Mail => Some(session.mail_account.as_str()),
            Class::Contacts => session.contacts(),
        };
        account
            .map(str::to_string)
            .ok_or(HandlerError::Jmap(JmapError::MissingCapability(
                eas_jmap::capability::CONTACTS,
            )))
    }

    /// IDs of the folder's items; `after` limits mail by date.
    pub async fn list(
        self,
        ctx: &Ctx<'_>,
        folder: &Folder,
        after: Option<&str>,
    ) -> Result<Vec<String>, HandlerError> {
        Ok(match self {
            Class::Mail => ctx.jmap.email_ids(&folder.jmap_id, after).await?,
            Class::Contacts => {
                ctx.jmap
                    .query_ids(
                        "ContactCard",
                        &self.account(ctx)?,
                        json!({ "inAddressBook": folder.jmap_id }),
                        Map::new(),
                        MAX_CONTACTS,
                    )
                    .await?
            }
        })
    }

    /// `/get` arguments for what [`Class::hash`] reads.
    pub fn hash_args(self) -> Value {
        match self {
            Class::Mail => json!({ "properties": email::HASH_PROPERTIES }),
            // Any change to a card matters to the device.
            Class::Contacts => json!({}),
        }
    }

    pub fn hash(self, item: &Value) -> String {
        match self {
            Class::Mail => email::state_hash(item),
            Class::Contacts => contact::hash(item),
        }
    }

    /// `ApplicationData` for a server-side change of an item fetched with
    /// [`Class::hash_args`].
    pub fn change_data(self, item: &Value) -> Element {
        match self {
            Class::Mail => email::change_data(item),
            Class::Contacts => contact::application_data(item),
        }
    }

    pub async fn get(
        self,
        ctx: &Ctx<'_>,
        ids: &[String],
        args: &Value,
    ) -> Result<Vec<Value>, HandlerError> {
        Ok(ctx
            .jmap
            .get(self.jmap_type(), &self.account(ctx)?, ids, args)
            .await?)
    }
}
