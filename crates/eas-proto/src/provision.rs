//! The `Provision` policy document (MS-ASPROV 2.2.2.27).
//!
//! The gateway hands out one policy for everyone. Its defaults ask nothing
//! of the device: the point of provisioning here is the policy key, which
//! remote wipe depends on, not device lockdown.

use eas_wbxml::{Element, Page};

/// The only policy type EAS 14.0 and later use.
pub const POLICY_TYPE: &str = "MS-EAS-Provisioning-WBXML";

/// Provision-level and policy-level status codes (MS-ASPROV 2.2.2.54).
pub mod status {
    pub const SUCCESS: u8 = 1;
    pub const PROTOCOL_ERROR: u8 = 2;
    pub const GENERAL_ERROR: u8 = 3;
    /// Policy level: the client asked for a policy type the server doesn't have.
    pub const UNKNOWN_POLICY_TYPE: u8 = 3;
    /// Policy level: the acknowledged key is not the one handed out.
    pub const INVALID_POLICY_KEY: u8 = 5;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Policy {
    pub require_password: bool,
    pub alphanumeric_password: bool,
    pub min_password_length: Option<u32>,
    /// Seconds until the device locks itself.
    pub max_inactivity_lock: Option<u32>,
    pub max_failed_attempts: Option<u32>,
    pub require_encryption: bool,
    pub allow_simple_password: bool,
}

impl Default for Policy {
    fn default() -> Policy {
        Policy {
            require_password: false,
            alphanumeric_password: false,
            min_password_length: None,
            max_inactivity_lock: None,
            max_failed_attempts: None,
            require_encryption: false,
            allow_simple_password: true,
        }
    }
}

fn flag(b: bool) -> &'static str {
    if b {
        "1"
    } else {
        "0"
    }
}

impl Policy {
    /// `EASProvisionDoc`, in the element order MS-ASPROV lists.
    pub fn document(&self) -> Element {
        let opt = |v: Option<u32>| v.map(|n| n.to_string()).unwrap_or_default();
        let fields: Vec<(&str, String)> = vec![
            ("DevicePasswordEnabled", flag(self.require_password).into()),
            (
                "AlphanumericDevicePasswordRequired",
                flag(self.alphanumeric_password).into(),
            ),
            ("AttachmentsEnabled", "1".into()),
            ("MinDevicePasswordLength", opt(self.min_password_length)),
            ("MaxInactivityTimeDeviceLock", opt(self.max_inactivity_lock)),
            (
                "MaxDevicePasswordFailedAttempts",
                opt(self.max_failed_attempts),
            ),
            ("MaxAttachmentSize", String::new()),
            (
                "AllowSimpleDevicePassword",
                flag(self.allow_simple_password).into(),
            ),
            ("DevicePasswordExpiration", String::new()),
            ("DevicePasswordHistory", "0".into()),
            ("AllowStorageCard", "1".into()),
            ("AllowCamera", "1".into()),
            (
                "RequireDeviceEncryption",
                flag(self.require_encryption).into(),
            ),
            ("AllowUnsignedApplications", "1".into()),
            ("AllowUnsignedInstallationPackages", "1".into()),
            ("MinDevicePasswordComplexCharacters", String::new()),
            ("AllowWiFi", "1".into()),
            ("AllowTextMessaging", "1".into()),
            ("AllowPOPIMAPEmail", "1".into()),
            ("AllowBluetooth", "2".into()),
            ("AllowIrDA", "1".into()),
            ("RequireManualSyncWhenRoaming", "0".into()),
            ("AllowDesktopSync", "1".into()),
            ("MaxCalendarAgeFilter", "0".into()),
            ("AllowHTMLEmail", "1".into()),
            ("MaxEmailAgeFilter", "0".into()),
            ("MaxEmailBodyTruncationSize", "-1".into()),
            ("MaxEmailHTMLBodyTruncationSize", "-1".into()),
            ("RequireSignedSMIMEMessages", "0".into()),
            ("RequireEncryptedSMIMEMessages", "0".into()),
            ("RequireSignedSMIMEAlgorithm", "0".into()),
            ("RequireEncryptionSMIMEAlgorithm", "0".into()),
            ("AllowSMIMEEncryptionAlgorithmNegotiation", "2".into()),
            ("AllowSMIMESoftCerts", "1".into()),
            ("AllowBrowser", "1".into()),
            ("AllowConsumerEmail", "1".into()),
            ("AllowRemoteDesktop", "1".into()),
            ("AllowInternetSharing", "1".into()),
        ];
        let mut doc = Element::new(Page::Provision, "EASProvisionDoc");
        for (name, value) in fields {
            // Tags missing from the code page tables are skipped rather than
            // guessed.
            let Ok(mut el) = Element::try_new(Page::Provision, name) else {
                continue;
            };
            if !value.is_empty() {
                el = el.with_text(value);
            }
            doc.push(el);
        }
        doc.push(Element::new(
            Page::Provision,
            "UnapprovedInROMApplicationList",
        ));
        doc.push(Element::new(Page::Provision, "ApprovedApplicationList"));
        doc
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_round_trips_through_wbxml() {
        let doc = Policy::default().document();
        assert_eq!(
            doc.child_text(Page::Provision, "DevicePasswordEnabled")
                .as_deref(),
            Some("0")
        );
        assert!(doc
            .child(Page::Provision, "MinDevicePasswordLength")
            .is_some());
        let bytes = eas_wbxml::encode(&doc);
        assert_eq!(eas_wbxml::decode(&bytes).unwrap(), doc);
    }
}
