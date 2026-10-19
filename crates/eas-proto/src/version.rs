use std::fmt;
use std::str::FromStr;

/// EAS protocol versions the gateway implements. Older versions (2.5, 12.x)
/// are deliberately unsupported; see docs/PLAN.md, "Scope".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ProtocolVersion {
    V14_0,
    V14_1,
    V16_0,
    V16_1,
}

impl ProtocolVersion {
    /// Supported versions, oldest first.
    pub const SUPPORTED: [ProtocolVersion; 4] = [
        ProtocolVersion::V14_0,
        ProtocolVersion::V14_1,
        ProtocolVersion::V16_0,
        ProtocolVersion::V16_1,
    ];

    pub const LATEST: ProtocolVersion = ProtocolVersion::V16_1;

    pub fn as_str(self) -> &'static str {
        match self {
            ProtocolVersion::V14_0 => "14.0",
            ProtocolVersion::V14_1 => "14.1",
            ProtocolVersion::V16_0 => "16.0",
            ProtocolVersion::V16_1 => "16.1",
        }
    }

    /// Version as the base64 request line encodes it: major and minor times
    /// ten (141 for 14.1).
    pub fn from_code(code: u8) -> Option<ProtocolVersion> {
        match code {
            140 => Some(ProtocolVersion::V14_0),
            141 => Some(ProtocolVersion::V14_1),
            160 => Some(ProtocolVersion::V16_0),
            161 => Some(ProtocolVersion::V16_1),
            _ => None,
        }
    }

    /// 16.x changes calendar semantics (server-sent invitations, instance
    /// edits) and adds drafts sync and `Find`.
    pub fn is_16(self) -> bool {
        self >= ProtocolVersion::V16_0
    }

    /// Comma-separated list for the `MS-ASProtocolVersions` header.
    pub fn header_list() -> String {
        ProtocolVersion::SUPPORTED
            .iter()
            .map(|v| v.as_str())
            .collect::<Vec<_>>()
            .join(",")
    }
}

impl fmt::Display for ProtocolVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The version string is not one the gateway supports.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unsupported ActiveSync protocol version {0:?}")]
pub struct UnsupportedVersion(pub String);

impl FromStr for ProtocolVersion {
    type Err = UnsupportedVersion;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        ProtocolVersion::SUPPORTED
            .into_iter()
            .find(|v| v.as_str() == s.trim())
            .ok_or_else(|| UnsupportedVersion(s.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_list() {
        assert_eq!("16.1".parse(), Ok(ProtocolVersion::V16_1));
        assert!("12.1".parse::<ProtocolVersion>().is_err());
        assert_eq!(ProtocolVersion::header_list(), "14.0,14.1,16.0,16.1");
        assert_eq!(
            ProtocolVersion::from_code(141),
            Some(ProtocolVersion::V14_1)
        );
        assert!(ProtocolVersion::V16_0.is_16() && !ProtocolVersion::V14_1.is_16());
    }
}
