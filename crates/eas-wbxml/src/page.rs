use crate::codepages::{NAMESPACES, PAGES};
use std::fmt;

/// EAS code pages (MS-ASWBXML 2.1.2.1). The discriminant is the page number
/// sent in `SWITCH_PAGE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum Page {
    AirSync = 0,
    Contacts = 1,
    Email = 2,
    AirNotify = 3,
    Calendar = 4,
    Move = 5,
    GetItemEstimate = 6,
    FolderHierarchy = 7,
    MeetingResponse = 8,
    Tasks = 9,
    ResolveRecipients = 10,
    ValidateCert = 11,
    Contacts2 = 12,
    Ping = 13,
    Provision = 14,
    Search = 15,
    Gal = 16,
    AirSyncBase = 17,
    Settings = 18,
    DocumentLibrary = 19,
    ItemOperations = 20,
    ComposeMail = 21,
    Email2 = 22,
    Notes = 23,
    RightsManagement = 24,
    Find = 25,
}

impl Page {
    /// All pages in number order.
    pub const ALL: [Page; 26] = [
        Page::AirSync,
        Page::Contacts,
        Page::Email,
        Page::AirNotify,
        Page::Calendar,
        Page::Move,
        Page::GetItemEstimate,
        Page::FolderHierarchy,
        Page::MeetingResponse,
        Page::Tasks,
        Page::ResolveRecipients,
        Page::ValidateCert,
        Page::Contacts2,
        Page::Ping,
        Page::Provision,
        Page::Search,
        Page::Gal,
        Page::AirSyncBase,
        Page::Settings,
        Page::DocumentLibrary,
        Page::ItemOperations,
        Page::ComposeMail,
        Page::Email2,
        Page::Notes,
        Page::RightsManagement,
        Page::Find,
    ];

    /// Page for a `SWITCH_PAGE` number, if it is an EAS page.
    pub fn from_number(n: u8) -> Option<Page> {
        Page::ALL.get(n as usize).copied()
    }

    pub fn number(self) -> u8 {
        self as u8
    }

    /// Namespace name as used in MS-ASWBXML and in XML renderings
    /// (`"AirSync"`, `"FolderHierarchy"`, …).
    pub fn namespace(self) -> &'static str {
        NAMESPACES[self as usize]
    }

    /// Page whose namespace is `ns` (case-sensitive).
    pub fn from_namespace(ns: &str) -> Option<Page> {
        NAMESPACES
            .iter()
            .position(|n| *n == ns)
            .and_then(|i| Page::from_number(i as u8))
    }

    /// Name of `token` on this page.
    pub fn tag_name(self, token: u8) -> Option<&'static str> {
        let table = PAGES[self as usize];
        table
            .binary_search_by_key(&token, |(t, _)| *t)
            .ok()
            .map(|i| table[i].1)
    }

    /// Token for `name` on this page. A few names appear twice in the tables
    /// (older and newer token assignments); the lowest token wins, the decoder
    /// accepts both.
    pub fn tag_token(self, name: &str) -> Option<u8> {
        PAGES[self as usize]
            .iter()
            .find(|(_, n)| *n == name)
            .map(|(t, _)| *t)
    }
}

impl fmt::Display for Page {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.namespace())
    }
}

/// A tag identity: code page plus 6-bit token (0x05..=0x3F).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Tag {
    pub page: Page,
    pub token: u8,
}

/// The tag name is not defined on that page.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown tag {name:?} on code page {page}")]
pub struct UnknownTag {
    pub page: Page,
    pub name: String,
}

impl Tag {
    /// Looks up `name` on `page`.
    pub fn named(page: Page, name: &str) -> Result<Tag, UnknownTag> {
        page.tag_token(name)
            .map(|token| Tag { page, token })
            .ok_or_else(|| UnknownTag {
                page,
                name: name.to_string(),
            })
    }

    /// Tag name, or `None` for a token the tables don't know.
    pub fn name(self) -> Option<&'static str> {
        self.page.tag_name(self.token)
    }
}

impl fmt::Display for Tag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.name() {
            Some(name) => write!(f, "{}:{}", self.page, name),
            None => write!(f, "{}:0x{:02X}", self.page, self.token),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_are_sorted_and_tokens_in_range() {
        for (i, table) in PAGES.iter().enumerate() {
            for pair in table.windows(2) {
                assert!(pair[0].0 < pair[1].0, "page {i} not sorted");
            }
            for (t, _) in table.iter() {
                assert!((0x05..=0x3F).contains(t), "page {i} token {t:#x}");
            }
        }
    }

    #[test]
    fn well_known_tokens() {
        assert_eq!(Page::AirSync.tag_token("Sync"), Some(0x05));
        assert_eq!(Page::AirSync.tag_token("Collections"), Some(0x1C));
        assert_eq!(Page::FolderHierarchy.tag_token("FolderSync"), Some(0x16));
        assert_eq!(Page::FolderHierarchy.tag_token("SyncKey"), Some(0x12));
        assert_eq!(Page::Ping.tag_token("HeartbeatInterval"), Some(0x08));
        assert_eq!(Page::Provision.tag_token("PolicyKey"), Some(0x09));
        assert_eq!(Page::AirSyncBase.tag_token("InstanceId"), Some(0x2D));
        assert_eq!(Page::Email.tag_name(0x0F), Some("DateReceived"));
    }

    #[test]
    fn namespaces_round_trip() {
        for page in Page::ALL {
            assert_eq!(Page::from_namespace(page.namespace()), Some(page));
            assert_eq!(Page::from_number(page.number()), Some(page));
        }
        assert_eq!(Page::from_number(26), None);
    }
}
