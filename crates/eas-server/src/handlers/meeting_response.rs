//! `MeetingResponse` (MS-ASCMD 2.2.1.11): accept, tentatively accept or
//! decline an invitation.
//!
//! The request names either the event in a calendar folder or the
//! invitation mail in a mail folder; for mail, the event is found by the
//! UID in the message's calendar part. The user's participation status is
//! set on the event and Stalwart sends the reply to the organiser
//! (`sendSchedulingMessages`). Responses to a single occurrence are
//! applied to the whole series for now.

use super::classes::{self, Class};
use super::folder_sync::KIND_CALENDAR;
use super::sync::server_id;
use super::{wbxml, Ctx, HandlerError, HandlerResult};
use eas_convert::calendar::{eas_to_status, participant_email};
use eas_jmap::{SetOp, SetOutcome};
use eas_wbxml::{Element, Page};
use serde_json::{json, Map, Value};

/// MeetingResponse status codes (MS-ASCMD 2.2.3.177.8).
mod status {
    pub const SUCCESS: u8 = 1;
    pub const INVALID_REQUEST: u8 = 2;
    pub const MAILBOX_ERROR: u8 = 3;
    pub const SERVER_ERROR: u8 = 4;
}

fn result(request_id: &str, code: u8, calendar_id: Option<String>) -> Element {
    let mut r = Element::new(Page::MeetingResponse, "Result")
        .with_text_child("RequestId", request_id)
        .with_text_child("Status", code.to_string());
    if let Some(c) = calendar_id {
        r = r.with_text_child("CalendarId", c);
    }
    r
}

/// The UID in a message's `text/calendar` part, unfolded.
pub fn calendar_uid(raw: &[u8]) -> Option<String> {
    let msg = mail_parser::MessageParser::default().parse(raw)?;
    for part in msg.parts.iter() {
        use mail_parser::MimeHeaders;
        let is_cal = part.content_type().is_some_and(|c| {
            c.ctype().eq_ignore_ascii_case("text")
                && c.subtype()
                    .is_some_and(|s| s.eq_ignore_ascii_case("calendar"))
        });
        if !is_cal {
            continue;
        }
        let text = String::from_utf8_lossy(part.contents())
            .replace("\r\n ", "")
            .replace("\n ", "");
        if let Some(uid) = text
            .lines()
            .find_map(|l| l.strip_prefix("UID:").or_else(|| l.strip_prefix("UID;")))
        {
            // `UID;X-PARAM=...:value` keeps the value after the last colon.
            let v = uid.rsplit(':').next().unwrap_or(uid).trim();
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

pub async fn handle(ctx: &Ctx<'_>, request: Option<Element>) -> HandlerResult {
    let req = request.ok_or(HandlerError::BadRequest("empty MeetingResponse"))?;
    let store = &ctx.state.store;
    let dev = ctx.device.id;
    let class = Class::Calendar;
    let mut out = Element::new(Page::MeetingResponse, "MeetingResponse");
    let Ok(account) = class.account(ctx) else {
        for r in req.children_named(Page::MeetingResponse, "Request") {
            let id = r
                .child_text(Page::MeetingResponse, "RequestId")
                .unwrap_or_default();
            out.push(result(&id, status::SERVER_ERROR, None));
        }
        return Ok(wbxml(&out));
    };
    let viewer = classes::viewer(ctx).await?;
    let folders = store.folders(dev).await?;

    for r in req.children_named(Page::MeetingResponse, "Request") {
        let request_id = r
            .child_text(Page::MeetingResponse, "RequestId")
            .unwrap_or_default();
        let response: u8 = r
            .child_text(Page::MeetingResponse, "UserResponse")
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(0);
        // EAS: 1 accepted, 2 tentative, 3 declined; attendee status codes
        // are 3, 2 and 4.
        let status_code = match response {
            1 => 3,
            2 => 2,
            3 => 4,
            _ => {
                out.push(result(&request_id, status::INVALID_REQUEST, None));
                continue;
            }
        };
        let cid = r
            .child_text(Page::MeetingResponse, "CollectionId")
            .unwrap_or_default();
        let Some(folder) = folders.iter().find(|f| f.server_id == cid) else {
            out.push(result(&request_id, status::INVALID_REQUEST, None));
            continue;
        };
        let jmap_id = request_id
            .split_once(':')
            .map(|(_, id)| id.to_string())
            .unwrap_or_else(|| request_id.clone());

        // Find the event: directly, or through the invitation's UID.
        let event_id = if folder.kind == KIND_CALENDAR {
            Some(jmap_id)
        } else {
            let emails = ctx
                .jmap
                .email_get(
                    std::slice::from_ref(&jmap_id),
                    &json!({"properties": ["blobId"]}),
                )
                .await?;
            let blob = emails
                .first()
                .and_then(|e| e.get("blobId"))
                .and_then(Value::as_str)
                .map(str::to_string);
            let uid = match blob {
                Some(b) => calendar_uid(&ctx.jmap.download(&b, "invite.eml").await?),
                None => None,
            };
            match uid {
                Some(uid) => ctx
                    .jmap
                    .query_ids(
                        "CalendarEvent",
                        &account,
                        json!({ "uid": uid }),
                        Map::new(),
                        1,
                    )
                    .await?
                    .into_iter()
                    .next(),
                None => None,
            }
        };
        let Some(event_id) = event_id else {
            out.push(result(&request_id, status::MAILBOX_ERROR, None));
            continue;
        };
        let events = class
            .get(ctx, std::slice::from_ref(&event_id), &json!({}))
            .await?;
        let Some(event) = events.first() else {
            out.push(result(&request_id, status::MAILBOX_ERROR, None));
            continue;
        };
        let me = event
            .get("participants")
            .and_then(Value::as_object)
            .and_then(|m| {
                m.iter().find(|(_, p)| {
                    let addr = participant_email(p);
                    viewer.me.iter().any(|x| x.eq_ignore_ascii_case(&addr))
                })
            })
            .map(|(id, _)| id.clone());
        let Some(me) = me else {
            out.push(result(&request_id, status::INVALID_REQUEST, None));
            continue;
        };
        let patch = json!({
            format!("participants/{me}/participationStatus"): eas_to_status(status_code),
            format!("participants/{me}/expectReply"): false,
        });
        let outcome = ctx
            .jmap
            .set_one(
                "CalendarEvent/set",
                &account,
                SetOp::Update(event_id.clone(), patch),
                json!({ "sendSchedulingMessages": true }),
            )
            .await?;
        if let SetOutcome::Failed(kind) = outcome {
            tracing::warn!(%kind, "meeting response failed");
            out.push(result(&request_id, status::SERVER_ERROR, None));
            continue;
        }
        tracing::info!(user = %ctx.device.username, response = eas_to_status(status_code), "meeting response sent");
        // The device shows the event in the calendar folder holding it.
        let calendar_id = event
            .get("calendarIds")
            .and_then(Value::as_object)
            .and_then(|m| {
                m.keys().find_map(|k| {
                    folders
                        .iter()
                        .find(|f| f.kind == KIND_CALENDAR && &f.jmap_id == k)
                })
            })
            .map(|f| server_id(&f.server_id, &event_id));
        let result_el = if status_code == 4 {
            // Declined: the device drops the item; no CalendarId.
            result(&request_id, status::SUCCESS, None)
        } else {
            result(&request_id, status::SUCCESS, calendar_id)
        };
        out.push(result_el);
    }
    Ok(wbxml(&out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_uid_in_invitation() {
        let raw = b"From: a@b\r\nTo: c@d\r\nSubject: Invitation\r\nMIME-Version: 1.0\r\nContent-Type: multipart/alternative; boundary=x\r\n\r\n--x\r\nContent-Type: text/plain\r\n\r\nHi\r\n--x\r\nContent-Type: text/calendar; method=REQUEST\r\n\r\nBEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:abc-123\r\n @example.org\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n--x--\r\n";
        assert_eq!(calendar_uid(raw).as_deref(), Some("abc-123@example.org"));
        assert_eq!(calendar_uid(b"Subject: x\r\n\r\nno calendar"), None);
    }
}
