//! Headers of the `OPTIONS /Microsoft-Server-ActiveSync` response
//! (MS-ASHTTP 2.2.4). Clients pick the highest version they share with the
//! server from `MS-ASProtocolVersions`.

use crate::command::Command;
use crate::version::ProtocolVersion;

/// Name and value of every header the `OPTIONS` response carries.
pub fn headers() -> Vec<(&'static str, String)> {
    vec![
        ("Allow", "OPTIONS,POST".to_string()),
        ("Public", "OPTIONS,POST".to_string()),
        ("MS-Server-ActiveSync", ProtocolVersion::LATEST.to_string()),
        ("MS-ASProtocolVersions", ProtocolVersion::header_list()),
        (
            "MS-ASProtocolCommands",
            Command::advertised()
                .map(Command::name)
                .collect::<Vec<_>>()
                .join(","),
        ),
    ]
}

#[cfg(test)]
mod tests {
    #[test]
    fn advertises_versions_and_commands() {
        let h = super::headers();
        let get = |n| {
            h.iter()
                .find(|(k, _)| *k == n)
                .map(|(_, v)| v.clone())
                .unwrap()
        };
        assert_eq!(get("MS-ASProtocolVersions"), "14.0,14.1,16.0,16.1");
        assert!(get("MS-ASProtocolCommands").starts_with("Sync,SendMail,"));
        assert!(!get("MS-ASProtocolCommands").contains("ValidateCert"));
    }
}
