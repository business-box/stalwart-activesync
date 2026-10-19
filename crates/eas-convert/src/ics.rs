//! Just enough iCalendar (RFC 5545) to describe an invitation mail: the
//! method and the first event's UID, times, summary, location, organiser
//! and sequence. Full event data comes from Stalwart when the event is in
//! a calendar; this is the fallback for invitations it hasn't filed.

use crate::calendar::EasEvent;
use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Invitation {
    /// `REQUEST`, `CANCEL`, `REPLY` ...
    pub method: String,
    pub uid: String,
    pub summary: Option<String>,
    pub location: Option<String>,
    pub organizer: Option<(String, String)>,
    pub start: Option<DateTime<Utc>>,
    pub end: Option<DateTime<Utc>>,
    pub all_day: bool,
    pub tz: Option<Tz>,
    pub recurring: bool,
    /// The replying attendee's PARTSTAT, for replies.
    pub partstat: Option<String>,
}

/// Unfolded content lines.
fn lines(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in text.split('\n') {
        let l = raw.trim_end_matches('\r');
        if let Some(cont) = l.strip_prefix(' ').or_else(|| l.strip_prefix('\t')) {
            if let Some(last) = out.last_mut() {
                last.push_str(cont);
                continue;
            }
        }
        out.push(l.to_string());
    }
    out
}

/// Name, parameters and value of a content line.
fn split(line: &str) -> (String, Vec<(String, String)>, String) {
    // The value starts at the first colon outside a quoted parameter.
    let mut quoted = false;
    let mut cut = line.len();
    for (i, c) in line.char_indices() {
        match c {
            '"' => quoted = !quoted,
            ':' if !quoted => {
                cut = i;
                break;
            }
            _ => {}
        }
    }
    let (head, value) = (&line[..cut], line.get(cut + 1..).unwrap_or_default());
    let mut parts = head.split(';');
    let name = parts.next().unwrap_or_default().to_ascii_uppercase();
    let params = parts
        .filter_map(|p| p.split_once('='))
        .map(|(k, v)| (k.to_ascii_uppercase(), v.trim_matches('"').to_string()))
        .collect();
    (name, params, value.to_string())
}

fn unescape(v: &str) -> String {
    v.replace("\\n", "\n")
        .replace("\\N", "\n")
        .replace("\\,", ",")
        .replace("\\;", ";")
        .replace("\\\\", "\\")
}

fn param<'a>(params: &'a [(String, String)], key: &str) -> Option<&'a str> {
    params
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

/// A DTSTART/DTEND value: UTC time, and whether it is a date only.
fn parse_time(
    params: &[(String, String)],
    value: &str,
) -> Option<(DateTime<Utc>, bool, Option<Tz>)> {
    let v = value.trim();
    if param(params, "VALUE") == Some("DATE") || v.len() == 8 {
        let d = NaiveDate::parse_from_str(v, "%Y%m%d").ok()?;
        return Some((Utc.from_utc_datetime(&d.and_hms_opt(0, 0, 0)?), true, None));
    }
    if let Some(z) = v.strip_suffix('Z') {
        let n = NaiveDateTime::parse_from_str(z, "%Y%m%dT%H%M%S").ok()?;
        return Some((Utc.from_utc_datetime(&n), false, None));
    }
    let n = NaiveDateTime::parse_from_str(v, "%Y%m%dT%H%M%S").ok()?;
    let tz: Option<Tz> = param(params, "TZID").and_then(|t| t.trim_start_matches('/').parse().ok());
    let utc = match tz {
        Some(tz) => tz
            .from_local_datetime(&n)
            .earliest()
            .map(|d| d.with_timezone(&Utc))?,
        // Unknown or Windows TZID: treat as UTC rather than guess.
        None => Utc.from_utc_datetime(&n),
    };
    Some((utc, false, tz))
}

pub fn parse(text: &str) -> Option<Invitation> {
    let mut inv = Invitation::default();
    let mut in_event = false;
    let mut seen_event = false;
    let mut duration = None;
    for line in lines(text) {
        let (name, params, value) = split(&line);
        match (name.as_str(), in_event) {
            ("METHOD", false) => inv.method = value.trim().to_ascii_uppercase(),
            ("BEGIN", _) if value.eq_ignore_ascii_case("VEVENT") => {
                if seen_event {
                    break;
                }
                in_event = true;
                seen_event = true;
            }
            ("END", true) if value.eq_ignore_ascii_case("VEVENT") => in_event = false,
            // Nested components (alarms) are skipped.
            ("BEGIN", true) => in_event = false,
            ("UID", true) => inv.uid = value.trim().to_string(),
            ("SUMMARY", true) => inv.summary = Some(unescape(&value)),
            ("LOCATION", true) => inv.location = Some(unescape(&value)).filter(|l| !l.is_empty()),
            ("ORGANIZER", true) => {
                let email = value
                    .trim()
                    .trim_start_matches("mailto:")
                    .trim_start_matches("MAILTO:");
                inv.organizer = Some((
                    param(&params, "CN").unwrap_or_default().to_string(),
                    email.to_string(),
                ));
            }
            ("DTSTART", true) => {
                if let Some((t, all_day, tz)) = parse_time(&params, &value) {
                    inv.start = Some(t);
                    inv.all_day = all_day;
                    inv.tz = tz;
                }
            }
            ("DTEND", true) => inv.end = parse_time(&params, &value).map(|(t, _, _)| t),
            ("DURATION", true) => duration = crate::calendar::parse_duration(&value),
            ("RRULE", true) => inv.recurring = true,
            ("ATTENDEE", true) => {
                if let Some(p) = param(&params, "PARTSTAT") {
                    inv.partstat = Some(p.to_ascii_uppercase());
                }
            }
            _ => {}
        }
    }
    if inv.end.is_none() {
        inv.end = match (inv.start, duration) {
            (Some(s), Some(d)) => Some(s + d),
            (Some(s), None) if inv.all_day => Some(s + Duration::days(1)),
            (s, _) => s,
        };
    }
    (!inv.uid.is_empty()).then_some(inv)
}

impl Invitation {
    /// The event as EAS sees it, from the invitation alone.
    pub fn to_event(&self) -> Option<EasEvent> {
        let start = self.start?;
        Some(EasEvent {
            tz: self.tz.unwrap_or(Tz::UTC),
            all_day: self.all_day,
            start,
            end: self.end.unwrap_or(start),
            subject: self.summary.clone(),
            location: self.location.clone(),
            body: None,
            reminder: None,
            busy: 2,
            sensitivity: 0,
            uid: Some(self.uid.clone()),
            organizer: self.organizer.clone(),
            attendees: Vec::new(),
            categories: Vec::new(),
            recurrence: None,
            exceptions: Vec::new(),
            cancelled: self.method == "CANCEL",
            dtstamp: None,
        })
    }

    /// The EAS message class for this invitation mail.
    pub fn message_class(&self) -> &'static str {
        match self.method.as_str() {
            "CANCEL" => "IPM.Schedule.Meeting.Canceled",
            "REPLY" => match self.partstat.as_deref() {
                Some("ACCEPTED") => "IPM.Schedule.Meeting.Resp.Pos",
                Some("DECLINED") => "IPM.Schedule.Meeting.Resp.Neg",
                _ => "IPM.Schedule.Meeting.Resp.Tent",
            },
            _ => "IPM.Schedule.Meeting.Request",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INVITE: &str = "BEGIN:VCALENDAR\r\nMETHOD:REQUEST\r\nBEGIN:VTIMEZONE\r\nTZID:Europe/Berlin\r\nEND:VTIMEZONE\r\nBEGIN:VEVENT\r\nUID:abc@example.org\r\nSUMMARY:Review\\, part 2\r\nLOCATION:Room 4\r\nORGANIZER;CN=\"Bob: the boss\":mailto:bob@example.org\r\nDTSTART;TZID=Europe/Berlin:20261020T140000\r\nDTEND;TZID=Europe/Berlin:20261020T150000\r\nRRULE:FREQ=WEEKLY\r\nATTENDEE;PARTSTAT=NEEDS-ACTION:mailto:alice@example.org\r\nBEGIN:VALARM\r\nTRIGGER:-PT15M\r\nEND:VALARM\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    #[test]
    fn parses_invitation() {
        let inv = parse(INVITE).unwrap();
        assert_eq!(inv.method, "REQUEST");
        assert_eq!(inv.uid, "abc@example.org");
        assert_eq!(inv.summary.as_deref(), Some("Review, part 2"));
        assert_eq!(
            inv.organizer,
            Some(("Bob: the boss".into(), "bob@example.org".into()))
        );
        assert_eq!(
            crate::calendar::eas_compact(inv.start.unwrap()),
            "20261020T120000Z"
        );
        assert_eq!(
            crate::calendar::eas_compact(inv.end.unwrap()),
            "20261020T130000Z"
        );
        assert!(inv.recurring);
        assert_eq!(inv.message_class(), "IPM.Schedule.Meeting.Request");
        let ev = inv.to_event().unwrap();
        assert_eq!(ev.tz.name(), "Europe/Berlin");
    }

    #[test]
    fn all_day_and_replies() {
        let ics = "BEGIN:VCALENDAR\nMETHOD:REPLY\nBEGIN:VEVENT\nUID:x\nDTSTART;VALUE=DATE:20261224\nATTENDEE;PARTSTAT=DECLINED:mailto:a@b\nEND:VEVENT\nEND:VCALENDAR\n";
        let inv = parse(ics).unwrap();
        assert!(inv.all_day);
        assert_eq!(
            crate::calendar::eas_compact(inv.end.unwrap()),
            "20261225T000000Z"
        );
        assert_eq!(inv.message_class(), "IPM.Schedule.Meeting.Resp.Neg");
        assert!(parse("BEGIN:VCALENDAR\nEND:VCALENDAR\n").is_none());
    }
}
