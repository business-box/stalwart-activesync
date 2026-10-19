//! The data classes the Sync engine handles and what differs between them:
//! which JMAP type backs a folder, how items are listed, fingerprinted and
//! serialised.

use super::folder_sync::{KIND_ADDRESS_BOOK, KIND_CALENDAR, KIND_MAILBOX};
use super::{Ctx, HandlerError};
use eas_convert::{calendar, contact, email};
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
    Calendar,
}

/// What serialising some classes needs beyond the item: the user's own
/// addresses (to tell organised meetings from received ones) and whether
/// the device speaks 16.x.
#[derive(Debug, Clone, Default)]
pub struct Viewer {
    pub me: Vec<String>,
    pub v16: bool,
}

impl Class {
    pub fn of(folder: &Folder) -> Option<Class> {
        match folder.kind.as_str() {
            KIND_MAILBOX => Some(Class::Mail),
            KIND_ADDRESS_BOOK => Some(Class::Contacts),
            KIND_CALENDAR => Some(Class::Calendar),
            _ => None,
        }
    }

    /// The JMAP type of the items.
    pub fn jmap_type(self) -> &'static str {
        match self {
            Class::Mail => "Email",
            Class::Contacts => "ContactCard",
            Class::Calendar => "CalendarEvent",
        }
    }

    pub fn account(self, ctx: &Ctx<'_>) -> Result<String, HandlerError> {
        let session = ctx.jmap.session();
        let account = match self {
            Class::Mail => Some(session.mail_account.as_str()),
            Class::Contacts => session.contacts(),
            Class::Calendar => session.calendars(),
        };
        let cap = match self {
            Class::Calendar => eas_jmap::capability::CALENDARS,
            _ => eas_jmap::capability::CONTACTS,
        };
        account
            .map(str::to_string)
            .ok_or(HandlerError::Jmap(JmapError::MissingCapability(cap)))
    }

    /// IDs of the folder's items; `after` limits mail and events by date.
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
            Class::Calendar => {
                let mut filter = Map::new();
                filter.insert("inCalendar".into(), json!(folder.jmap_id));
                if let Some(a) = after {
                    // Events (or occurrences) ending after the window start.
                    filter.insert("after".into(), json!(a));
                }
                ctx.jmap
                    .query_ids(
                        "CalendarEvent",
                        &self.account(ctx)?,
                        Value::Object(filter),
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
            // Any change to a card or event matters to the device.
            Class::Contacts | Class::Calendar => json!({}),
        }
    }

    pub fn hash(self, item: &Value) -> String {
        match self {
            Class::Mail => email::state_hash(item),
            Class::Contacts | Class::Calendar => contact::hash(item),
        }
    }

    /// `ApplicationData` for a server-side change of an item fetched with
    /// [`Class::hash_args`].
    pub fn change_data(self, item: &Value, viewer: &Viewer) -> Element {
        match self {
            Class::Mail => email::change_data(item),
            Class::Contacts => contact::application_data(item),
            Class::Calendar => calendar::application_data(item, &viewer.me, viewer.v16)
                .unwrap_or_else(|| Element::new(eas_wbxml::Page::AirSync, "ApplicationData")),
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

/// The user's addresses: identities (without wildcards) and the login name.
pub async fn viewer(ctx: &Ctx<'_>) -> Result<Viewer, HandlerError> {
    let mut me: Vec<String> = ctx
        .jmap
        .identities()
        .await?
        .into_iter()
        .map(|i| i.email)
        .filter(|e| !e.is_empty() && !e.starts_with('*'))
        .collect();
    let user = &ctx.jmap.session().username;
    if user.contains('@') && !me.iter().any(|m| m.eq_ignore_ascii_case(user)) {
        me.push(user.clone());
    }
    Ok(Viewer {
        me,
        v16: ctx.version.is_16(),
    })
}
