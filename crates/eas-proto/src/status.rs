//! Status codes shared by all commands (MS-ASCMD 2.2.2) and the root
//! element a command-level status goes into.

use crate::command::Command;
use eas_wbxml::{Element, Page};

pub const SUCCESS: u8 = 1;
pub const INVALID_CONTENT: u8 = 101;
pub const INVALID_WBXML: u8 = 102;
pub const SERVER_ERROR: u8 = 110;
pub const MAILBOX_QUOTA_EXCEEDED: u8 = 113;
pub const ACCESS_DENIED: u8 = 126;
pub const DEVICE_BLOCKED: u8 = 129;
pub const REMOTE_WIPE_REQUESTED: u8 = 140;
pub const DEVICE_NOT_PROVISIONED: u8 = 142;
pub const POLICY_REFRESH: u8 = 143;
pub const INVALID_POLICY_KEY: u8 = 144;

impl Command {
    /// Root element of the command's response and the page its `Status`
    /// lives on, for commands that carry a top-level status. Commands without
    /// one (`GetAttachment`, `MoveItems`, `MeetingResponse`) get HTTP 449
    /// when a policy is missing.
    pub fn status_root(self) -> Option<(Page, &'static str)> {
        Some(match self {
            Command::Sync => (Page::AirSync, "Sync"),
            Command::FolderSync => (Page::FolderHierarchy, "FolderSync"),
            Command::FolderCreate => (Page::FolderHierarchy, "FolderCreate"),
            Command::FolderUpdate => (Page::FolderHierarchy, "FolderUpdate"),
            Command::FolderDelete => (Page::FolderHierarchy, "FolderDelete"),
            Command::GetItemEstimate => (Page::GetItemEstimate, "GetItemEstimate"),
            Command::Ping => (Page::Ping, "Ping"),
            Command::Settings => (Page::Settings, "Settings"),
            Command::Search => (Page::Search, "Search"),
            Command::ItemOperations => (Page::ItemOperations, "ItemOperations"),
            Command::ResolveRecipients => (Page::ResolveRecipients, "ResolveRecipients"),
            Command::SendMail => (Page::ComposeMail, "SendMail"),
            Command::SmartReply => (Page::ComposeMail, "SmartReply"),
            Command::SmartForward => (Page::ComposeMail, "SmartForward"),
            Command::Find => (Page::Find, "Find"),
            Command::Provision => (Page::Provision, "Provision"),
            Command::GetAttachment
            | Command::MoveItems
            | Command::MeetingResponse
            | Command::ValidateCert => return None,
        })
    }

    /// `<Root><Status>code</Status></Root>` for this command, if it has a
    /// top-level status.
    pub fn status_response(self, code: u8) -> Option<Element> {
        let (page, root) = self.status_root()?;
        Some(Element::new(page, root).with_text_child("Status", code.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_status_root_has_a_status_tag() {
        for c in Command::ALL {
            if let Some(el) = c.status_response(DEVICE_NOT_PROVISIONED) {
                assert_eq!(el.child_text(el.page(), "Status").as_deref(), Some("142"));
            }
        }
        assert!(Command::GetAttachment.status_response(142).is_none());
    }
}
