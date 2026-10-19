//! `Settings` (MS-ASCMD 2.2.1.18): out-of-office through JMAP
//! `VacationResponse`, device information, the user's addresses.

use super::{wbxml, Ctx, HandlerError, HandlerResult};
use eas_convert::email::eas_date;
use eas_jmap::mail::Vacation;
use eas_proto::ProtocolVersion;
use eas_wbxml::{Element, Page};

/// Settings status codes (MS-ASCMD 2.2.3.177.14).
mod status {
    pub const SUCCESS: u8 = 1;
    pub const PROTOCOL_ERROR: u8 = 2;
    pub const DENIED: u8 = 3;
    pub const SERVER_ERROR: u8 = 4;
    pub const INVALID_ARGUMENTS: u8 = 5;
}

fn with_status(name: &str, code: u8) -> Element {
    Element::new(Page::Settings, name).with_text_child("Status", code.to_string())
}

/// EAS `2026-10-19T08:00:00.000Z` to JMAP `2026-10-19T08:00:00Z`.
fn jmap_date(eas: &str) -> Option<String> {
    let dt = chrono::DateTime::parse_from_rfc3339(eas.trim()).ok()?;
    Some(
        dt.with_timezone(&chrono::Utc)
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string(),
    )
}

fn oof_get(v: &Vacation, html: bool) -> Element {
    let state = match (v.is_enabled, v.from_date.is_some() || v.to_date.is_some()) {
        (false, _) => "0",
        (true, false) => "1",
        (true, true) => "2",
    };
    let mut get = Element::new(Page::Settings, "Get").with_text_child("OofState", state);
    if state == "2" {
        if let Some(d) = v.from_date.as_deref().and_then(eas_date) {
            get = get.with_text_child("StartTime", d);
        }
        if let Some(d) = v.to_date.as_deref().and_then(eas_date) {
            get = get.with_text_child("EndTime", d);
        }
    }
    let (body, body_type) = match (html, &v.html_body, &v.text_body) {
        (true, Some(h), _) => (h.clone(), "HTML"),
        (_, _, Some(t)) => (t.clone(), "TEXT"),
        (_, Some(h), None) => (eas_convert::text::html_to_text(h), "TEXT"),
        _ => (String::new(), "TEXT"),
    };
    // JMAP has one message; EAS asks per audience. Same text for all three.
    for audience in [
        "AppliesToInternal",
        "AppliesToExternalKnown",
        "AppliesToExternalUnknown",
    ] {
        get.push(
            Element::new(Page::Settings, "OofMessage")
                .with_child(Element::new(Page::Settings, audience))
                .with_text_child("Enabled", if v.is_enabled { "1" } else { "0" })
                .with_text_child("ReplyMessage", body.clone())
                .with_text_child("BodyType", body_type),
        );
    }
    get
}

/// Applies an `Oof/Set`. `Err` carries the status for a bad request.
fn oof_set(set: &Element, current: &Vacation) -> Result<Vacation, u8> {
    let state = set
        .child_text(Page::Settings, "OofState")
        .map(|s| s.trim().to_string());
    let mut v = current.clone();
    match state.as_deref() {
        Some("0") => {
            v.is_enabled = false;
            return Ok(v);
        }
        Some("1") => {
            v.is_enabled = true;
            v.from_date = None;
            v.to_date = None;
        }
        Some("2") => {
            v.is_enabled = true;
            let start = set.child_text(Page::Settings, "StartTime");
            let end = set.child_text(Page::Settings, "EndTime");
            v.from_date = start
                .as_deref()
                .map(jmap_date)
                .ok_or(status::INVALID_ARGUMENTS)?;
            v.to_date = end
                .as_deref()
                .map(jmap_date)
                .ok_or(status::INVALID_ARGUMENTS)?;
            if v.from_date.is_none() || v.to_date.is_none() {
                return Err(status::INVALID_ARGUMENTS);
            }
        }
        // A Set without a state only changes the message.
        None => {}
        Some(_) => return Err(status::INVALID_ARGUMENTS),
    }
    // Prefer the internal message; fall back to any enabled one.
    let messages: Vec<&Element> = set.children_named(Page::Settings, "OofMessage").collect();
    let chosen = messages
        .iter()
        .find(|m| m.child(Page::Settings, "AppliesToInternal").is_some())
        .or_else(|| {
            messages
                .iter()
                .find(|m| m.child_text(Page::Settings, "Enabled").as_deref() == Some("1"))
        });
    if let Some(m) = chosen {
        if let Some(text) = m.child_text(Page::Settings, "ReplyMessage") {
            let html = m
                .child_text(Page::Settings, "BodyType")
                .is_some_and(|t| t.eq_ignore_ascii_case("html"));
            if html {
                v.text_body = Some(eas_convert::text::html_to_text(&text));
                v.html_body = Some(text);
            } else {
                v.text_body = Some(text);
                v.html_body = None;
            }
        }
    }
    Ok(v)
}

pub async fn handle(ctx: &Ctx<'_>, request: Option<Element>) -> HandlerResult {
    let req = request.ok_or(HandlerError::BadRequest("empty Settings"))?;
    if !req.is(Page::Settings, "Settings") {
        return Ok(wbxml(&with_status("Settings", status::PROTOCOL_ERROR)));
    }
    let mut out = with_status("Settings", status::SUCCESS);
    for item in req.elements() {
        match item.name() {
            "Oof" => {
                if let Some(get) = item.child(Page::Settings, "Get") {
                    let html = get
                        .child_text(Page::Settings, "BodyType")
                        .is_some_and(|t| t.eq_ignore_ascii_case("html"));
                    let v = ctx.jmap.vacation().await?;
                    out.push(with_status("Oof", status::SUCCESS).with_child(oof_get(&v, html)));
                } else if let Some(set) = item.child(Page::Settings, "Set") {
                    let current = ctx.jmap.vacation().await?;
                    let code = match oof_set(set, &current) {
                        Ok(v) => match ctx.jmap.set_vacation(&v).await {
                            Ok(()) => status::SUCCESS,
                            Err(e) => {
                                tracing::warn!(error = %e, "VacationResponse/set failed");
                                status::SERVER_ERROR
                            }
                        },
                        Err(code) => code,
                    };
                    out.push(with_status("Oof", code));
                } else {
                    out.push(with_status("Oof", status::PROTOCOL_ERROR));
                }
            }
            "DeviceInformation" => {
                if let Some(set) = item.child(Page::Settings, "Set") {
                    ctx.state
                        .store
                        .set_device_info(
                            ctx.device.id,
                            set.child_text(Page::Settings, "Model").as_deref(),
                            set.child_text(Page::Settings, "FriendlyName").as_deref(),
                            set.child_text(Page::Settings, "OS").as_deref(),
                        )
                        .await?;
                }
                out.push(with_status("DeviceInformation", status::SUCCESS));
            }
            "UserInformation" => {
                let identities = ctx.jmap.identities().await?;
                let username = ctx.jmap.session().username.clone();
                let mut addrs: Vec<String> = identities
                    .iter()
                    .map(|i| i.email.clone())
                    .filter(|e| !e.is_empty() && !e.starts_with('*'))
                    .collect();
                if addrs.is_empty() && username.contains('@') {
                    addrs.push(username.clone());
                }
                let primary = addrs
                    .iter()
                    .find(|a| a.eq_ignore_ascii_case(&username))
                    .or(addrs.first())
                    .cloned()
                    .unwrap_or_default();
                let display = identities
                    .iter()
                    .find(|i| i.email.eq_ignore_ascii_case(&primary))
                    .map(|i| i.name.clone())
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| primary.clone());
                let mut emails = Element::new(Page::Settings, "EmailAddresses");
                for a in &addrs {
                    emails.push(Element::new(Page::Settings, "SmtpAddress").with_text(a.clone()));
                }
                let get = if ctx.version >= ProtocolVersion::V14_1 {
                    emails.push(
                        Element::new(Page::Settings, "PrimarySmtpAddress").with_text(primary),
                    );
                    Element::new(Page::Settings, "Get").with_child(
                        Element::new(Page::Settings, "Accounts").with_child(
                            Element::new(Page::Settings, "Account")
                                .with_text_child("UserDisplayName", display)
                                .with_child(emails),
                        ),
                    )
                } else {
                    Element::new(Page::Settings, "Get").with_child(emails)
                };
                out.push(with_status("UserInformation", status::SUCCESS).with_child(get));
            }
            // No device password recovery and no rights management: accept
            // and ignore, as Exchange does when the features are off.
            "DevicePassword" => out.push(with_status("DevicePassword", status::SUCCESS)),
            "RightsManagementInformation" => out.push(
                with_status("RightsManagementInformation", status::SUCCESS)
                    .with_child(Element::new(Page::Settings, "Get")),
            ),
            other => {
                tracing::debug!(setting = other, "unknown Settings item");
                out.push(Element {
                    tag: item.tag,
                    children: vec![eas_wbxml::Node::Element(
                        Element::new(Page::Settings, "Status")
                            .with_text(status::DENIED.to_string()),
                    )],
                });
            }
        }
    }
    Ok(wbxml(&out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(name: &str, v: &str) -> Element {
        Element::new(Page::Settings, name).with_text(v)
    }

    #[test]
    fn oof_round_trip() {
        let set = Element::new(Page::Settings, "Set")
            .with_child(t("OofState", "2"))
            .with_child(t("StartTime", "2026-10-20T08:00:00.000Z"))
            .with_child(t("EndTime", "2026-10-27T17:00:00.000Z"))
            .with_child(
                Element::new(Page::Settings, "OofMessage")
                    .with_child(Element::new(Page::Settings, "AppliesToInternal"))
                    .with_child(t("Enabled", "1"))
                    .with_child(t("ReplyMessage", "Away"))
                    .with_child(t("BodyType", "Text")),
            );
        let v = oof_set(&set, &Vacation::default()).unwrap();
        assert!(v.is_enabled);
        assert_eq!(v.from_date.as_deref(), Some("2026-10-20T08:00:00Z"));
        assert_eq!(v.text_body.as_deref(), Some("Away"));
        let get = oof_get(&v, false);
        assert_eq!(
            get.child_text(Page::Settings, "OofState").as_deref(),
            Some("2")
        );
        assert_eq!(
            get.child_text(Page::Settings, "StartTime").as_deref(),
            Some("2026-10-20T08:00:00.000Z")
        );
        assert_eq!(get.children_named(Page::Settings, "OofMessage").count(), 3);

        let off = Element::new(Page::Settings, "Set").with_child(t("OofState", "0"));
        let v = oof_set(&off, &v).unwrap();
        assert!(!v.is_enabled);
        assert_eq!(v.text_body.as_deref(), Some("Away"), "message kept");

        let bad = Element::new(Page::Settings, "Set").with_child(t("OofState", "2"));
        assert_eq!(oof_set(&bad, &v).unwrap_err(), status::INVALID_ARGUMENTS);
    }
}
