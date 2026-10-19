use std::fmt;

/// EAS commands, with the codes used by the base64 request line
/// (MS-ASHTTP 2.2.1.1.1.1.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Command {
    Sync,
    SendMail,
    SmartForward,
    SmartReply,
    GetAttachment,
    FolderSync,
    FolderCreate,
    FolderDelete,
    FolderUpdate,
    MoveItems,
    GetItemEstimate,
    MeetingResponse,
    Search,
    Settings,
    Ping,
    ItemOperations,
    Provision,
    ResolveRecipients,
    ValidateCert,
    Find,
}

impl Command {
    pub const ALL: [Command; 20] = [
        Command::Sync,
        Command::SendMail,
        Command::SmartForward,
        Command::SmartReply,
        Command::GetAttachment,
        Command::FolderSync,
        Command::FolderCreate,
        Command::FolderDelete,
        Command::FolderUpdate,
        Command::MoveItems,
        Command::GetItemEstimate,
        Command::MeetingResponse,
        Command::Search,
        Command::Settings,
        Command::Ping,
        Command::ItemOperations,
        Command::Provision,
        Command::ResolveRecipients,
        Command::ValidateCert,
        Command::Find,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Command::Sync => "Sync",
            Command::SendMail => "SendMail",
            Command::SmartForward => "SmartForward",
            Command::SmartReply => "SmartReply",
            Command::GetAttachment => "GetAttachment",
            Command::FolderSync => "FolderSync",
            Command::FolderCreate => "FolderCreate",
            Command::FolderDelete => "FolderDelete",
            Command::FolderUpdate => "FolderUpdate",
            Command::MoveItems => "MoveItems",
            Command::GetItemEstimate => "GetItemEstimate",
            Command::MeetingResponse => "MeetingResponse",
            Command::Search => "Search",
            Command::Settings => "Settings",
            Command::Ping => "Ping",
            Command::ItemOperations => "ItemOperations",
            Command::Provision => "Provision",
            Command::ResolveRecipients => "ResolveRecipients",
            Command::ValidateCert => "ValidateCert",
            Command::Find => "Find",
        }
    }

    pub fn code(self) -> u8 {
        match self {
            Command::Sync => 0,
            Command::SendMail => 1,
            Command::SmartForward => 2,
            Command::SmartReply => 3,
            Command::GetAttachment => 4,
            Command::FolderSync => 9,
            Command::FolderCreate => 10,
            Command::FolderDelete => 11,
            Command::FolderUpdate => 12,
            Command::MoveItems => 13,
            Command::GetItemEstimate => 14,
            Command::MeetingResponse => 15,
            Command::Search => 16,
            Command::Settings => 17,
            Command::Ping => 18,
            Command::ItemOperations => 19,
            Command::Provision => 20,
            Command::ResolveRecipients => 21,
            Command::ValidateCert => 22,
            Command::Find => 23,
        }
    }

    pub fn from_code(code: u8) -> Option<Command> {
        Command::ALL.into_iter().find(|c| c.code() == code)
    }

    /// Case-insensitive, because some clients vary the casing of `Cmd`.
    pub fn from_name(name: &str) -> Option<Command> {
        Command::ALL
            .into_iter()
            .find(|c| c.name().eq_ignore_ascii_case(name))
    }

    /// Commands advertised in `MS-ASProtocolCommands`. `ValidateCert` is out
    /// of scope (no S/MIME certificate handling), so it is not offered.
    pub fn advertised() -> impl Iterator<Item = Command> {
        Command::ALL
            .into_iter()
            .filter(|c| *c != Command::ValidateCert)
    }

    /// Commands that must be rejected with HTTP 449 until the device has a
    /// valid policy key; everything except `Provision` itself.
    pub fn requires_policy(self) -> bool {
        !matches!(self, Command::Provision)
    }
}

impl fmt::Display for Command {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_and_names_are_unique() {
        for a in Command::ALL {
            assert_eq!(Command::from_code(a.code()), Some(a));
            assert_eq!(Command::from_name(a.name()), Some(a));
            assert_eq!(Command::from_name(&a.name().to_lowercase()), Some(a));
        }
        assert_eq!(Command::from_code(5), None);
    }
}
