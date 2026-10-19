//! JMAP `CalendarEvent` (JSCalendar, with the JSCalendar-bis names
//! Stalwart uses) to and from EAS calendar items (MS-ASCAL).
//!
//! Both sides go through [`EasEvent`], the event as EAS sees it. A device
//! change is turned back into JMAP by comparing the device's `EasEvent`
//! with the one projected from the stored event, group by group (times,
//! title, location, recurrence, exceptions, attendees ...), and patching
//! only the groups that differ. Properties EAS can't express are left
//! alone; recurrence rules EAS can't express degrade to the nearest EAS
//! pattern when sent, and are only rewritten if the device changes them.

use crate::tz::{self, TimeZoneInfo};
use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;
use eas_wbxml::{Element, Page};
use serde_json::{json, Map, Value};

/// EAS calendar dates: `20261019T130000Z`.
pub fn eas_compact(t: DateTime<Utc>) -> String {
    t.format("%Y%m%dT%H%M%SZ").to_string()
}

pub fn parse_compact(s: &str) -> Option<DateTime<Utc>> {
    let s = s.trim();
    NaiveDateTime::parse_from_str(s, "%Y%m%dT%H%M%SZ")
        .ok()
        .or_else(|| {
            // Some clients send the extended form.
            DateTime::parse_from_rfc3339(s)
                .ok()
                .map(|d| d.with_timezone(&Utc).naive_utc())
        })
        .map(|n| Utc.from_utc_datetime(&n))
}

/// ISO 8601 durations as JSCalendar uses them (`P1D`, `PT1H30M`, `P2W`).
pub fn parse_duration(s: &str) -> Option<Duration> {
    let s = s.trim().trim_start_matches('+');
    let (neg, s) = match s.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, s),
    };
    let s = s.strip_prefix('P')?;
    let mut total = Duration::zero();
    let mut num = String::new();
    let mut in_time = false;
    for c in s.chars() {
        match c {
            'T' => in_time = true,
            '0'..='9' | '.' => num.push(c),
            _ => {
                let n: f64 = num.parse().ok()?;
                num.clear();
                let secs = match (c, in_time) {
                    ('W', false) => n * 7.0 * 86400.0,
                    ('D', false) => n * 86400.0,
                    ('H', true) => n * 3600.0,
                    ('M', true) => n * 60.0,
                    ('S', true) => n,
                    _ => return None,
                };
                total += Duration::milliseconds((secs * 1000.0) as i64);
            }
        }
    }
    Some(if neg { -total } else { total })
}

pub fn format_duration(d: Duration) -> String {
    let mut secs = d.num_seconds().max(0);
    if secs == 0 {
        return "PT0S".into();
    }
    let days = secs / 86400;
    secs %= 86400;
    let mut out = String::from("P");
    if days > 0 {
        out.push_str(&format!("{days}D"));
    }
    if secs > 0 {
        out.push('T');
        let (h, m, s) = (secs / 3600, secs % 3600 / 60, secs % 60);
        if h > 0 {
            out.push_str(&format!("{h}H"));
        }
        if m > 0 {
            out.push_str(&format!("{m}M"));
        }
        if s > 0 {
            out.push_str(&format!("{s}S"));
        }
    }
    out
}

const LOCAL: &str = "%Y-%m-%dT%H:%M:%S";

fn parse_local(s: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(s.trim(), LOCAL).ok()
}

fn to_utc(tz: Tz, local: NaiveDateTime) -> DateTime<Utc> {
    tz.from_local_datetime(&local)
        .earliest()
        // A time skipped by the spring change: take it an hour later.
        .or_else(|| {
            tz.from_local_datetime(&(local + Duration::hours(1)))
                .earliest()
        })
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|| Utc.from_utc_datetime(&local))
}

fn to_local(tz: Tz, t: DateTime<Utc>) -> String {
    t.with_timezone(&tz).naive_local().format(LOCAL).to_string()
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Recurrence {
    /// 0 daily, 1 weekly, 2 monthly, 3 monthly on the nth weekday,
    /// 5 yearly, 6 yearly on the nth weekday.
    pub kind: u8,
    pub interval: u32,
    pub occurrences: Option<u32>,
    pub until: Option<DateTime<Utc>>,
    /// Sunday = 1, Monday = 2, ... Saturday = 64.
    pub day_of_week: u8,
    pub day_of_month: Option<u32>,
    /// 1 to 4, or 5 for the last.
    pub week_of_month: Option<u8>,
    pub month_of_year: Option<u32>,
    /// 0 = Sunday.
    pub first_day_of_week: Option<u8>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Attendee {
    pub email: String,
    pub name: String,
    /// 0 unknown, 2 tentative, 3 accepted, 4 declined, 5 not responded.
    pub status: u8,
    /// 1 required, 2 optional, 3 resource.
    pub kind: u8,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Exception {
    /// Start of the occurrence this exception replaces.
    pub original_start: DateTime<Utc>,
    pub deleted: bool,
    pub start: Option<DateTime<Utc>>,
    pub end: Option<DateTime<Utc>>,
    pub subject: Option<String>,
    pub location: Option<String>,
}

/// An event as EAS sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EasEvent {
    pub tz: Tz,
    pub all_day: bool,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub subject: Option<String>,
    pub location: Option<String>,
    pub body: Option<String>,
    /// Minutes before the start.
    pub reminder: Option<i64>,
    /// 0 free, 1 tentative, 2 busy, 3 out of office, 4 elsewhere.
    pub busy: u8,
    /// 0 normal, 2 private, 3 confidential.
    pub sensitivity: u8,
    pub uid: Option<String>,
    pub organizer: Option<(String, String)>,
    pub attendees: Vec<Attendee>,
    pub categories: Vec<String>,
    pub recurrence: Option<Recurrence>,
    pub exceptions: Vec<Exception>,
    pub cancelled: bool,
    pub dtstamp: Option<DateTime<Utc>>,
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or_default()
}

fn flag(v: &Value, map: &str, key: &str) -> bool {
    v.get(map)
        .and_then(|m| m.get(key))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// A participant's address, from `calendarAddress`, `sendTo` or `email`.
pub fn participant_email(p: &Value) -> String {
    let addr = match s(p, "calendarAddress") {
        "" => p
            .get("sendTo")
            .and_then(|t| t.get("imip"))
            .and_then(Value::as_str)
            .unwrap_or_else(|| s(p, "email")),
        a => a,
    };
    addr.trim()
        .trim_start_matches("mailto:")
        .trim_start_matches("MAILTO:")
        .to_string()
}

const DAYS: [&str; 7] = ["su", "mo", "tu", "we", "th", "fr", "sa"];

fn day_bit(day: &str) -> u8 {
    DAYS.iter()
        .position(|d| d.eq_ignore_ascii_case(day))
        .map(|i| 1u8 << i)
        .unwrap_or(0)
}

fn rule_to_eas(r: &Value, tz: Tz, start: DateTime<Utc>) -> Recurrence {
    let local_start = start.with_timezone(&tz);
    let by_day: Vec<&Value> = r
        .get("byDay")
        .and_then(Value::as_array)
        .map(|a| a.iter().collect())
        .unwrap_or_default();
    let mask: u8 = by_day
        .iter()
        .map(|d| day_bit(s(d, "day")))
        .fold(0, |a, b| a | b);
    let nth = by_day
        .iter()
        .find_map(|d| d.get("nthOfPeriod").and_then(Value::as_i64))
        .or_else(|| {
            r.get("bySetPosition")
                .and_then(Value::as_array)
                .and_then(|a| a.first())
                .and_then(Value::as_i64)
        })
        .map(|n| if n < 0 { 5 } else { n.clamp(1, 5) as u8 });
    let month_day = r
        .get("byMonthDay")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(Value::as_i64)
        .map(|d| d as u32);
    let month = r
        .get("byMonth")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(|m| {
            m.as_str()
                .and_then(|m| m.trim_end_matches('L').parse().ok())
                .or(m.as_u64().map(|m| m as u32))
        });
    let weekday_bit = 1u8 << local_start.weekday().num_days_from_sunday();
    let mut rec = Recurrence {
        interval: r.get("interval").and_then(Value::as_u64).unwrap_or(1) as u32,
        occurrences: r.get("count").and_then(Value::as_u64).map(|c| c as u32),
        until: r
            .get("until")
            .and_then(Value::as_str)
            .and_then(parse_local)
            .map(|u| to_utc(tz, u)),
        first_day_of_week: r
            .get("firstDayOfWeek")
            .and_then(Value::as_str)
            .and_then(|d| DAYS.iter().position(|x| x.eq_ignore_ascii_case(d)))
            .map(|i| i as u8),
        ..Default::default()
    };
    match s(r, "frequency") {
        "weekly" => {
            rec.kind = 1;
            rec.day_of_week = if mask == 0 { weekday_bit } else { mask };
        }
        "monthly" if mask != 0 => {
            rec.kind = 3;
            rec.day_of_week = mask;
            rec.week_of_month = Some(nth.unwrap_or(1));
        }
        "monthly" => {
            rec.kind = 2;
            rec.day_of_month = Some(month_day.unwrap_or(local_start.day()));
        }
        "yearly" if mask != 0 => {
            rec.kind = 6;
            rec.day_of_week = mask;
            rec.week_of_month = Some(nth.unwrap_or(1));
            rec.month_of_year = Some(month.unwrap_or(local_start.month()));
        }
        "yearly" => {
            rec.kind = 5;
            rec.day_of_month = Some(month_day.unwrap_or(local_start.day()));
            rec.month_of_year = Some(month.unwrap_or(local_start.month()));
        }
        // Daily, and anything finer, which EAS can't express.
        _ => {
            rec.kind = 0;
            rec.day_of_week = mask;
        }
    }
    rec
}

fn eas_to_rule(r: &Recurrence, tz: Tz) -> Value {
    let freq = match r.kind {
        0 => "daily",
        1 => "weekly",
        2 | 3 => "monthly",
        _ => "yearly",
    };
    let mut v = json!({"@type": "RecurrenceRule", "frequency": freq});
    if r.interval > 1 {
        v["interval"] = json!(r.interval);
    }
    if let Some(c) = r.occurrences {
        v["count"] = json!(c);
    }
    if let Some(u) = r.until {
        v["until"] = json!(to_local(tz, u));
    }
    let days: Vec<&str> = (0..7)
        .filter(|i| r.day_of_week & (1 << i) != 0)
        .map(|i| DAYS[i])
        .collect();
    let nth = r.week_of_month.map(|w| if w == 5 { -1 } else { w as i64 });
    match r.kind {
        3 | 6 if days.len() == 1 => {
            v["byDay"] =
                json!([{"@type": "NDay", "day": days[0], "nthOfPeriod": nth.unwrap_or(1)}]);
        }
        3 | 6 => {
            v["byDay"] = json!(days
                .iter()
                .map(|d| json!({"@type": "NDay", "day": d}))
                .collect::<Vec<_>>());
            v["bySetPosition"] = json!([nth.unwrap_or(1)]);
        }
        _ if !days.is_empty() => {
            v["byDay"] = json!(days
                .iter()
                .map(|d| json!({"@type": "NDay", "day": d}))
                .collect::<Vec<_>>());
        }
        _ => {}
    }
    if let Some(d) = r.day_of_month {
        v["byMonthDay"] = json!([d]);
    }
    if let Some(m) = r.month_of_year {
        v["byMonth"] = json!([m.to_string()]);
    }
    if let Some(f) = r.first_day_of_week {
        v["firstDayOfWeek"] = json!(DAYS[(f % 7) as usize]);
    }
    v
}

fn status_to_eas(st: &str) -> u8 {
    match st {
        "accepted" => 3,
        "declined" => 4,
        "tentative" => 2,
        "needs-action" => 5,
        _ => 0,
    }
}

/// EAS response or attendee status to a JSCalendar participation status.
pub fn eas_to_status(st: u8) -> &'static str {
    match st {
        3 => "accepted",
        4 => "declined",
        2 => "tentative",
        _ => "needs-action",
    }
}

/// The recurrence rule, under either name.
fn rule_of(event: &Value) -> Option<&Value> {
    event
        .get("recurrenceRule")
        .filter(|r| r.is_object())
        .or_else(|| {
            event
                .get("recurrenceRules")
                .and_then(Value::as_array)
                .and_then(|a| a.first())
        })
}

impl EasEvent {
    /// Projects a JMAP event.
    pub fn from_jmap(e: &Value) -> Option<EasEvent> {
        let tz: Tz = e
            .get("timeZone")
            .and_then(Value::as_str)
            .and_then(|z| z.parse().ok())
            .unwrap_or(Tz::UTC);
        let local = parse_local(s(e, "start"))?;
        let start = to_utc(tz, local);
        let all_day = e.get("showWithoutTime").and_then(Value::as_bool) == Some(true);
        let duration = e
            .get("duration")
            .and_then(Value::as_str)
            .and_then(parse_duration)
            .unwrap_or_else(|| {
                if all_day {
                    Duration::days(1)
                } else {
                    Duration::zero()
                }
            });
        let end = if all_day {
            // Whole days, so a DST change inside doesn't shift the end.
            to_utc(tz, local + duration)
        } else {
            start + duration
        };
        let location = e
            .get("locations")
            .and_then(Value::as_object)
            .and_then(|m| {
                let mut v: Vec<(&String, &Value)> = m.iter().collect();
                v.sort_by_key(|(k, _)| k.as_str());
                v.into_iter()
                    .map(|(_, l)| s(l, "name"))
                    .find(|n| !n.is_empty())
            })
            .map(str::to_string);
        let reminder = e.get("alerts").and_then(Value::as_object).and_then(|m| {
            m.values().find_map(|a| {
                let t = a.get("trigger")?;
                if s(t, "relativeTo") == "end" {
                    return None;
                }
                let off = parse_duration(t.get("offset")?.as_str()?)?;
                (off <= Duration::zero()).then(|| -off.num_minutes())
            })
        });
        let organizer_addr = s(e, "organizerCalendarAddress")
            .trim_start_matches("mailto:")
            .to_string();
        let parts: Vec<&Value> = e
            .get("participants")
            .and_then(Value::as_object)
            .map(|m| {
                let mut v: Vec<(&String, &Value)> = m.iter().collect();
                v.sort_by_key(|(k, _)| k.as_str());
                v.into_iter().map(|(_, p)| p).collect()
            })
            .unwrap_or_default();
        let is_owner = |p: &Value| {
            flag(p, "roles", "owner")
                || (!organizer_addr.is_empty()
                    && participant_email(p).eq_ignore_ascii_case(&organizer_addr))
        };
        let organizer = parts
            .iter()
            .find(|p| is_owner(p))
            .map(|p| (s(p, "name").to_string(), participant_email(p)))
            .or_else(|| {
                (!organizer_addr.is_empty()).then(|| (String::new(), organizer_addr.clone()))
            });
        let attendees = parts
            .iter()
            .filter(|p| !is_owner(p) || flag(p, "roles", "attendee") && parts.len() == 1)
            .filter(|p| !is_owner(p))
            .map(|p| Attendee {
                email: participant_email(p),
                name: s(p, "name").to_string(),
                status: status_to_eas(s(p, "participationStatus")),
                kind: if matches!(s(p, "kind"), "resource" | "location") {
                    3
                } else if flag(p, "roles", "optional") {
                    2
                } else {
                    1
                },
            })
            .collect();
        let mut categories: Vec<String> = e
            .get("keywords")
            .and_then(Value::as_object)
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default();
        categories.sort();
        let recurrence = rule_of(e).map(|r| rule_to_eas(r, tz, start));
        let mut exceptions: Vec<Exception> = e
            .get("recurrenceOverrides")
            .and_then(Value::as_object)
            .map(|m| {
                m.iter()
                    .filter_map(|(rid, patch)| {
                        let original = to_utc(tz, parse_local(rid)?);
                        if patch.get("excluded").and_then(Value::as_bool) == Some(true) {
                            return Some(Exception {
                                original_start: original,
                                deleted: true,
                                ..Default::default()
                            });
                        }
                        let new_start = patch
                            .get("start")
                            .and_then(Value::as_str)
                            .and_then(parse_local)
                            .map(|l| to_utc(tz, l));
                        let dur = patch
                            .get("duration")
                            .and_then(Value::as_str)
                            .and_then(parse_duration);
                        let st = new_start.unwrap_or(original);
                        Some(Exception {
                            original_start: original,
                            deleted: false,
                            start: Some(st),
                            end: Some(st + dur.unwrap_or(end - start)),
                            subject: patch
                                .get("title")
                                .and_then(Value::as_str)
                                .map(str::to_string),
                            location: patch
                                .get("locations")
                                .and_then(Value::as_object)
                                .and_then(|m| {
                                    m.values().map(|l| s(l, "name")).find(|n| !n.is_empty())
                                })
                                .map(str::to_string),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        exceptions.sort_by_key(|x| x.original_start);
        let busy = match (s(e, "freeBusyStatus"), s(e, "status")) {
            ("free", _) => 0,
            (_, "tentative") => 1,
            _ => 2,
        };
        Some(EasEvent {
            tz,
            all_day,
            start,
            end,
            subject: Some(s(e, "title").to_string()).filter(|t| !t.is_empty()),
            location,
            body: Some(s(e, "description").to_string()).filter(|t| !t.is_empty()),
            reminder,
            busy,
            sensitivity: match s(e, "privacy") {
                "private" => 2,
                "secret" => 3,
                _ => 0,
            },
            uid: Some(s(e, "uid").to_string()).filter(|u| !u.is_empty()),
            organizer,
            attendees,
            categories,
            recurrence,
            exceptions,
            cancelled: s(e, "status") == "cancelled",
            dtstamp: [s(e, "updated"), s(e, "created")]
                .iter()
                .find_map(|d| DateTime::parse_from_rfc3339(d).ok())
                .map(|d| d.with_timezone(&Utc)),
        })
    }

    /// `ApplicationData` for the device. `me` are the user's addresses,
    /// to tell organised meetings from received ones.
    pub fn to_element(&self, me: &[String], v16: bool) -> Element {
        let c = |name: &str, v: String| Element::new(Page::Calendar, name).with_text(v);
        let mut d = Element::new(Page::AirSync, "ApplicationData");
        let year = self.start.with_timezone(&self.tz).year();
        d.push(c("TimeZone", tz::info_for(self.tz, year).encode()));
        d.push(c("AllDayEvent", (self.all_day as u8).to_string()));
        if let Some(b) = &self.body {
            d.push(
                Element::new(Page::AirSyncBase, "Body")
                    .with_text_child("Type", "1")
                    .with_text_child("EstimatedDataSize", b.len().to_string())
                    .with_text_child("Data", b.clone()),
            );
        }
        d.push(c("BusyStatus", self.busy.to_string()));
        if let Some((name, email)) = &self.organizer {
            if !name.is_empty() {
                d.push(c("OrganizerName", name.clone()));
            }
            d.push(c("OrganizerEmail", email.clone()));
        }
        d.push(c(
            "DTStamp",
            eas_compact(self.dtstamp.unwrap_or(self.start)),
        ));
        d.push(c("EndTime", eas_compact(self.end)));
        if let Some(l) = &self.location {
            if v16 {
                d.push(
                    Element::new(Page::AirSyncBase, "Location")
                        .with_text_child("DisplayName", l.clone()),
                );
            } else {
                d.push(c("Location", l.clone()));
            }
        }
        if let Some(r) = self.reminder {
            d.push(c("Reminder", r.to_string()));
        }
        d.push(c("Sensitivity", self.sensitivity.to_string()));
        d.push(c("Subject", self.subject.clone().unwrap_or_default()));
        d.push(c("StartTime", eas_compact(self.start)));
        if let Some(u) = &self.uid {
            d.push(c("UID", u.clone()));
        }
        let mine = |a: &str| me.iter().any(|m| m.eq_ignore_ascii_case(a));
        let organised = self.organizer.as_ref().map_or(true, |(_, e)| mine(e));
        let meeting = !self.attendees.is_empty();
        let status = match (meeting, organised, self.cancelled) {
            (false, _, _) => 0,
            (true, true, false) => 1,
            (true, true, true) => 5,
            (true, false, false) => 3,
            (true, false, true) => 7,
        };
        d.push(c("MeetingStatus", status.to_string()));
        if meeting {
            let mut atts = Element::new(Page::Calendar, "Attendees");
            for a in &self.attendees {
                let mut el = Element::new(Page::Calendar, "Attendee")
                    .with_text_child("Email", a.email.clone())
                    .with_text_child(
                        "Name",
                        if a.name.is_empty() {
                            a.email.clone()
                        } else {
                            a.name.clone()
                        },
                    );
                el = el
                    .with_text_child("AttendeeStatus", a.status.to_string())
                    .with_text_child("AttendeeType", a.kind.to_string());
                atts.push(el);
            }
            d.push(atts);
        }
        if !self.categories.is_empty() {
            let mut cats = Element::new(Page::Calendar, "Categories");
            for cat in &self.categories {
                cats.push(c("Category", cat.clone()));
            }
            d.push(cats);
        }
        if let Some(r) = &self.recurrence {
            let mut rec = Element::new(Page::Calendar, "Recurrence")
                .with_text_child("Recurrence_Type", r.kind.to_string());
            if let Some(o) = r.occurrences {
                rec = rec.with_text_child("Recurrence_Occurrences", o.to_string());
            }
            rec = rec.with_text_child("Recurrence_Interval", r.interval.max(1).to_string());
            if let Some(u) = r.until {
                rec = rec.with_text_child("Recurrence_Until", eas_compact(u));
            }
            if r.day_of_week != 0 {
                rec = rec.with_text_child("Recurrence_DayOfWeek", r.day_of_week.to_string());
            }
            if let Some(dm) = r.day_of_month {
                rec = rec.with_text_child("Recurrence_DayOfMonth", dm.to_string());
            }
            if let Some(w) = r.week_of_month {
                rec = rec.with_text_child("Recurrence_WeekOfMonth", w.to_string());
            }
            if let Some(m) = r.month_of_year {
                rec = rec.with_text_child("Recurrence_MonthOfYear", m.to_string());
            }
            if let Some(f) = r.first_day_of_week {
                rec = rec.with_text_child("FirstDayOfWeek", f.to_string());
            }
            d.push(rec);
        }
        if !self.exceptions.is_empty() {
            let mut exs = Element::new(Page::Calendar, "Exceptions");
            for x in &self.exceptions {
                let mut el = Element::new(Page::Calendar, "Exception");
                if x.deleted {
                    el.push(c("Deleted", "1".into()));
                }
                el.push(c("ExceptionStartTime", eas_compact(x.original_start)));
                if !x.deleted {
                    if let Some(st) = x.start {
                        el.push(c("StartTime", eas_compact(st)));
                    }
                    if let Some(en) = x.end {
                        el.push(c("EndTime", eas_compact(en)));
                    }
                    if let Some(sub) = &x.subject {
                        el.push(c("Subject", sub.clone()));
                    }
                    if let Some(l) = &x.location {
                        el.push(c("Location", l.clone()));
                    }
                }
                exs.push(el);
            }
            d.push(exs);
        }
        let my_status = self
            .attendees
            .iter()
            .find(|a| mine(&a.email))
            .map(|a| a.status);
        if meeting {
            let response = if organised { 1 } else { my_status.unwrap_or(5) };
            d.push(c("ResponseType", response.to_string()));
            if !organised {
                d.push(c("ResponseRequested", "1".into()));
            }
        }
        d
    }

    /// Reads a device's `ApplicationData`. Fields it leaves out come from
    /// `base` (the stored event) where clients are known to omit them
    /// unchanged; without a base they are empty.
    pub fn from_device(
        data: &Element,
        base: Option<&EasEvent>,
        year_hint: i32,
    ) -> Option<EasEvent> {
        let t = |name: &str| {
            data.child_text(Page::Calendar, name)
                .map(|v| v.trim().to_string())
        };
        let tz_info = t("TimeZone").and_then(|b| TimeZoneInfo::decode(&b));
        let start = t("StartTime")
            .and_then(|v| parse_compact(&v))
            .or(base.map(|b| b.start))?;
        let end = t("EndTime")
            .and_then(|v| parse_compact(&v))
            .or(base.map(|b| b.end))
            .unwrap_or(start);
        let tz = match &tz_info {
            Some(info) => tz::to_iana(info, year_hint).unwrap_or(Tz::UTC),
            None => base.map(|b| b.tz).unwrap_or(Tz::UTC),
        };
        let all_day = t("AllDayEvent")
            .map(|v| v == "1")
            .or(base.map(|b| b.all_day))
            .unwrap_or(false);
        let body = data
            .child(Page::AirSyncBase, "Body")
            .and_then(|b| b.child_text(Page::AirSyncBase, "Data"))
            .or_else(|| t("Body"))
            .or_else(|| base.and_then(|b| b.body.clone()));
        let location = data
            .child(Page::AirSyncBase, "Location")
            .and_then(|l| l.child_text(Page::AirSyncBase, "DisplayName"))
            .or_else(|| t("Location"))
            .filter(|l| !l.is_empty());
        let attendees = match data.child(Page::Calendar, "Attendees") {
            Some(a) => a
                .children_named(Page::Calendar, "Attendee")
                .map(|x| Attendee {
                    email: x
                        .child_text(Page::Calendar, "Email")
                        .unwrap_or_default()
                        .trim()
                        .to_string(),
                    name: x.child_text(Page::Calendar, "Name").unwrap_or_default(),
                    status: x
                        .child_text(Page::Calendar, "AttendeeStatus")
                        .and_then(|v| v.trim().parse().ok())
                        .unwrap_or(0),
                    kind: x
                        .child_text(Page::Calendar, "AttendeeType")
                        .and_then(|v| v.trim().parse().ok())
                        .unwrap_or(1),
                })
                .filter(|a| !a.email.is_empty())
                .map(|mut a| {
                    // Nameless attendees go out with their address as name.
                    if a.name.trim().eq_ignore_ascii_case(&a.email) {
                        a.name.clear();
                    }
                    a
                })
                .collect(),
            None => base.map(|b| b.attendees.clone()).unwrap_or_default(),
        };
        let categories = match data.child(Page::Calendar, "Categories") {
            Some(c) => {
                let mut v: Vec<String> = c
                    .children_named(Page::Calendar, "Category")
                    .filter_map(Element::text)
                    .collect();
                v.sort();
                v
            }
            None => base.map(|b| b.categories.clone()).unwrap_or_default(),
        };
        let recurrence = data.child(Page::Calendar, "Recurrence").map(|r| {
            let n = |name: &str| {
                r.child_text(Page::Calendar, name)
                    .and_then(|v| v.trim().parse::<u32>().ok())
            };
            Recurrence {
                kind: n("Recurrence_Type").unwrap_or(0) as u8,
                interval: n("Recurrence_Interval").unwrap_or(1).max(1),
                occurrences: n("Recurrence_Occurrences"),
                until: r
                    .child_text(Page::Calendar, "Recurrence_Until")
                    .and_then(|v| parse_compact(&v)),
                day_of_week: n("Recurrence_DayOfWeek").unwrap_or(0) as u8,
                day_of_month: n("Recurrence_DayOfMonth"),
                week_of_month: n("Recurrence_WeekOfMonth").map(|w| w as u8),
                month_of_year: n("Recurrence_MonthOfYear"),
                first_day_of_week: n("FirstDayOfWeek").map(|f| f as u8),
            }
        });
        let exceptions = match data.child(Page::Calendar, "Exceptions") {
            Some(xs) => {
                let mut v: Vec<Exception> = xs
                    .children_named(Page::Calendar, "Exception")
                    .filter_map(|x| {
                        let xt = |name: &str| {
                            x.child_text(Page::Calendar, name)
                                .map(|v| v.trim().to_string())
                        };
                        let original = xt("ExceptionStartTime").and_then(|v| parse_compact(&v))?;
                        Some(Exception {
                            original_start: original,
                            deleted: xt("Deleted").as_deref() == Some("1"),
                            start: xt("StartTime").and_then(|v| parse_compact(&v)),
                            end: xt("EndTime").and_then(|v| parse_compact(&v)),
                            subject: xt("Subject"),
                            location: xt("Location").or_else(|| {
                                x.child(Page::AirSyncBase, "Location")
                                    .and_then(|l| l.child_text(Page::AirSyncBase, "DisplayName"))
                            }),
                        })
                    })
                    .collect();
                v.sort_by_key(|x| x.original_start);
                v
            }
            None => base.map(|b| b.exceptions.clone()).unwrap_or_default(),
        };
        Some(EasEvent {
            tz,
            all_day,
            start,
            end,
            subject: t("Subject")
                .or_else(|| base.and_then(|b| b.subject.clone()))
                .filter(|v| !v.is_empty()),
            location,
            body: body.filter(|b| !b.is_empty()),
            reminder: t("Reminder").and_then(|v| v.parse().ok()),
            busy: t("BusyStatus")
                .and_then(|v| v.parse().ok())
                .or(base.map(|b| b.busy))
                .unwrap_or(2),
            sensitivity: t("Sensitivity")
                .and_then(|v| v.parse().ok())
                .or(base.map(|b| b.sensitivity))
                .unwrap_or(0),
            uid: t("UID")
                .or_else(|| t("ClientUid"))
                .or_else(|| base.and_then(|b| b.uid.clone())),
            organizer: base.and_then(|b| b.organizer.clone()),
            attendees,
            categories,
            recurrence,
            exceptions,
            cancelled: base.is_some_and(|b| b.cancelled),
            dtstamp: base.and_then(|b| b.dtstamp),
        })
    }

    fn time_props(&self) -> Map<String, Value> {
        let mut m = Map::new();
        if self.all_day {
            // All-day events are floating dates in JSCalendar.
            let first = self.start.with_timezone(&self.tz).date_naive();
            let last = self.end.with_timezone(&self.tz).date_naive();
            let days = (last - first).num_days().max(1);
            m.insert(
                "start".into(),
                json!(first
                    .and_hms_opt(0, 0, 0)
                    .unwrap()
                    .format(LOCAL)
                    .to_string()),
            );
            m.insert("timeZone".into(), Value::Null);
            m.insert("duration".into(), json!(format!("P{days}D")));
            m.insert("showWithoutTime".into(), json!(true));
        } else {
            m.insert("start".into(), json!(to_local(self.tz, self.start)));
            m.insert("timeZone".into(), json!(self.tz.name()));
            m.insert(
                "duration".into(),
                json!(format_duration(self.end - self.start)),
            );
            m.insert("showWithoutTime".into(), json!(false));
        }
        m
    }

    fn override_key(&self, original: DateTime<Utc>) -> String {
        if self.all_day {
            original
                .with_timezone(&self.tz)
                .date_naive()
                .and_hms_opt(0, 0, 0)
                .unwrap()
                .format(LOCAL)
                .to_string()
        } else {
            to_local(self.tz, original)
        }
    }

    fn overrides(&self, existing: Option<&Value>) -> Value {
        let mut out = Map::new();
        for x in &self.exceptions {
            let key = self.override_key(x.original_start);
            if x.deleted {
                out.insert(key, json!({"excluded": true}));
                continue;
            }
            // Keep what EAS can't express in an existing override.
            let mut patch = existing
                .and_then(|m| m.get(&key))
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            patch.remove("excluded");
            let start = x.start.unwrap_or(x.original_start);
            if start != x.original_start {
                patch.insert("start".into(), json!(to_local(self.tz, start)));
            } else {
                patch.remove("start");
            }
            let dur = x.end.map(|e| e - start).unwrap_or(self.end - self.start);
            if dur != self.end - self.start {
                patch.insert("duration".into(), json!(format_duration(dur)));
            } else {
                patch.remove("duration");
            }
            match &x.subject {
                Some(t) if Some(t) != self.subject.as_ref() => {
                    patch.insert("title".into(), json!(t));
                }
                _ => {
                    patch.remove("title");
                }
            }
            match &x.location {
                Some(l) if Some(l) != self.location.as_ref() => {
                    patch.insert(
                        "locations".into(),
                        json!({"eas": {"@type": "Location", "name": l}}),
                    );
                }
                _ => {
                    patch.remove("locations");
                }
            }
            out.insert(key, Value::Object(patch));
        }
        Value::Object(out)
    }

    fn participants(&self, existing: Option<&Value>, me: &[String]) -> Value {
        let mut out = Map::new();
        let old: Vec<(String, Value)> = existing
            .and_then(Value::as_object)
            .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();
        // The organiser stays as it was; a new meeting gets the user.
        match old.iter().find(|(_, p)| flag(p, "roles", "owner")) {
            Some((id, p)) => {
                out.insert(id.clone(), p.clone());
            }
            None => {
                if let Some(addr) = me.first() {
                    out.insert(
                        "eas-organizer".into(),
                        json!({
                            "@type": "Participant",
                            "calendarAddress": format!("mailto:{addr}"),
                            "roles": {"owner": true, "attendee": true},
                            "participationStatus": "accepted",
                        }),
                    );
                }
            }
        }
        for (i, a) in self.attendees.iter().enumerate() {
            let existing = old
                .iter()
                .find(|(_, p)| participant_email(p).eq_ignore_ascii_case(&a.email));
            let (id, mut p) = match existing {
                Some((id, p)) => (id.clone(), p.clone()),
                None => (
                    format!("eas-{i}"),
                    json!({
                        "@type": "Participant",
                        "calendarAddress": format!("mailto:{}", a.email),
                        "participationStatus": "needs-action",
                        "expectReply": true,
                    }),
                ),
            };
            if !a.name.is_empty() && a.name != a.email {
                p["name"] = json!(a.name);
            }
            let mut roles = p
                .get("roles")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            roles.insert("attendee".into(), json!(true));
            if a.kind == 2 {
                roles.insert("optional".into(), json!(true));
            } else {
                roles.remove("optional");
            }
            p["roles"] = Value::Object(roles);
            if a.kind == 3 {
                p["kind"] = json!("resource");
            }
            out.insert(id, p);
        }
        if self.attendees.is_empty() {
            return Value::Null;
        }
        Value::Object(out)
    }

    fn simple_props(&self) -> Map<String, Value> {
        let mut m = Map::new();
        m.insert(
            "title".into(),
            json!(self.subject.clone().unwrap_or_default()),
        );
        m.insert(
            "description".into(),
            self.body.clone().map(Value::String).unwrap_or(Value::Null),
        );
        m.insert(
            "locations".into(),
            match &self.location {
                Some(l) => json!({"eas": {"@type": "Location", "name": l}}),
                None => Value::Null,
            },
        );
        m.insert(
            "alerts".into(),
            match self.reminder {
                Some(r) => json!({"eas": {
                    "@type": "Alert",
                    "trigger": {"@type": "OffsetTrigger", "offset": format!("-PT{}M", r.max(0)), "relativeTo": "start"},
                    "action": "display",
                }}),
                None => Value::Null,
            },
        );
        m.insert(
            "freeBusyStatus".into(),
            json!(if self.busy == 0 { "free" } else { "busy" }),
        );
        m.insert(
            "privacy".into(),
            json!(match self.sensitivity {
                2 => "private",
                3 => "secret",
                _ => "public",
            }),
        );
        m.insert(
            "keywords".into(),
            if self.categories.is_empty() {
                Value::Null
            } else {
                Value::Object(
                    self.categories
                        .iter()
                        .map(|c| (c.clone(), json!(true)))
                        .collect(),
                )
            },
        );
        m
    }

    /// A new JMAP event for a device `Add`.
    pub fn to_new_jmap(&self, calendar: &str, me: &[String]) -> Value {
        let mut m = self.time_props();
        for (k, v) in self.simple_props() {
            if !v.is_null() {
                m.insert(k, v);
            }
        }
        m.insert("@type".into(), json!("Event"));
        m.insert("calendarIds".into(), json!({ calendar: true }));
        if let Some(u) = &self.uid {
            m.insert("uid".into(), json!(u));
        }
        if let Some(r) = &self.recurrence {
            m.insert("recurrenceRule".into(), eas_to_rule(r, self.tz));
            if !self.exceptions.is_empty() {
                m.insert("recurrenceOverrides".into(), self.overrides(None));
            }
        }
        let parts = self.participants(None, me);
        if !parts.is_null() {
            m.insert("participants".into(), parts);
            if let Some(addr) = me.first() {
                m.insert(
                    "organizerCalendarAddress".into(),
                    json!(format!("mailto:{addr}")),
                );
            }
        }
        Value::Object(m)
    }

    /// Patch from `current` (the stored event, projected) to `self` (what
    /// the device sent), touching only groups that differ.
    pub fn patch(&self, current: &EasEvent, stored: &Value, me: &[String]) -> Map<String, Value> {
        let mut out = Map::new();
        if (self.start, self.end, self.all_day) != (current.start, current.end, current.all_day)
            || (!self.all_day && self.tz != current.tz)
        {
            out.extend(self.time_props());
        }
        let mine = self.simple_props();
        let theirs = current.simple_props();
        for (k, v) in mine {
            if theirs.get(&k) != Some(&v) {
                out.insert(k, v);
            }
        }
        if self.recurrence != current.recurrence {
            out.insert(
                "recurrenceRule".into(),
                self.recurrence
                    .as_ref()
                    .map(|r| eas_to_rule(r, self.tz))
                    .unwrap_or(Value::Null),
            );
            if stored.get("recurrenceRules").is_some() {
                out.insert("recurrenceRules".into(), Value::Null);
            }
        }
        if self.exceptions != current.exceptions || out.contains_key("start") {
            let ov = self.overrides(stored.get("recurrenceOverrides"));
            out.insert(
                "recurrenceOverrides".into(),
                if ov.as_object().is_some_and(|m| m.is_empty()) {
                    Value::Null
                } else {
                    ov
                },
            );
        }
        if self.attendees != current.attendees {
            out.insert(
                "participants".into(),
                self.participants(stored.get("participants"), me),
            );
        }
        out
    }
}

/// `ApplicationData` for a stored event.
pub fn application_data(event: &Value, me: &[String], v16: bool) -> Option<Element> {
    Some(EasEvent::from_jmap(event)?.to_element(me, v16))
}

/// The year to read device time zones against.
pub fn year_hint() -> i32 {
    Utc::now().year()
}

/// Midnight UTC of a date, for tests and fallbacks.
pub fn utc_midnight(y: i32, m: u32, d: u32) -> Option<DateTime<Utc>> {
    NaiveDate::from_ymd_opt(y, m, d)
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .map(|n| Utc.from_utc_datetime(&n))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn me() -> Vec<String> {
        vec!["alice@example.org".into()]
    }

    fn weekly_meeting() -> Value {
        json!({
            "@type": "Event",
            "uid": "u1@example.org",
            "title": "Standup",
            "description": "Daily sync",
            "start": "2026-10-19T09:30:00",
            "timeZone": "Europe/Berlin",
            "duration": "PT15M",
            "locations": {"l1": {"name": "Room 1"}},
            "alerts": {"a1": {"trigger": {"@type": "OffsetTrigger", "offset": "-PT10M"}}},
            "recurrenceRule": {"frequency": "weekly", "byDay": [{"day": "mo"}, {"day": "we"}], "until": "2026-12-31T23:59:59"},
            "recurrenceOverrides": {
                "2026-10-21T09:30:00": {"excluded": true},
                "2026-10-26T09:30:00": {"start": "2026-10-26T10:00:00", "title": "Standup (late)", "x-unknown": 1}
            },
            "organizerCalendarAddress": "mailto:alice@example.org",
            "participants": {
                "p0": {"calendarAddress": "mailto:alice@example.org", "roles": {"owner": true, "attendee": true}, "participationStatus": "accepted"},
                "p1": {"calendarAddress": "mailto:bob@example.org", "name": "Bob", "roles": {"attendee": true}, "participationStatus": "tentative"},
                "p2": {"calendarAddress": "mailto:room@example.org", "kind": "resource", "roles": {"attendee": true}}
            },
            "freeBusyStatus": "busy",
            "privacy": "private",
            "keywords": {"team": true}
        })
    }

    #[test]
    fn durations_and_dates() {
        assert_eq!(parse_duration("PT1H30M"), Some(Duration::minutes(90)));
        assert_eq!(parse_duration("P1DT2H"), Some(Duration::hours(26)));
        assert_eq!(parse_duration("-PT15M"), Some(Duration::minutes(-15)));
        assert_eq!(parse_duration("P2W"), Some(Duration::days(14)));
        assert_eq!(format_duration(Duration::minutes(90)), "PT1H30M");
        assert_eq!(format_duration(Duration::days(2)), "P2D");
        let t = parse_compact("20261019T073000Z").unwrap();
        assert_eq!(eas_compact(t), "20261019T073000Z");
        assert_eq!(parse_compact("2026-10-19T07:30:00.000Z"), Some(t));
    }

    #[test]
    fn projects_meeting() {
        let e = EasEvent::from_jmap(&weekly_meeting()).unwrap();
        // 09:30 in Berlin summer time is 07:30 UTC.
        assert_eq!(eas_compact(e.start), "20261019T073000Z");
        assert_eq!(eas_compact(e.end), "20261019T074500Z");
        assert_eq!(e.reminder, Some(10));
        assert_eq!(e.organizer.as_ref().unwrap().1, "alice@example.org");
        assert_eq!(e.attendees.len(), 2);
        assert_eq!(e.attendees[0].status, 2);
        assert_eq!(e.attendees[1].kind, 3);
        let r = e.recurrence.as_ref().unwrap();
        assert_eq!((r.kind, r.day_of_week), (1, 2 | 8));
        assert_eq!(e.exceptions.len(), 2);
        assert!(e.exceptions[0].deleted);
        assert_eq!(
            eas_compact(e.exceptions[1].start.unwrap()),
            "20261026T090000Z"
        );

        let d = e.to_element(&me(), false);
        let t = |n: &str| d.child_text(Page::Calendar, n);
        assert_eq!(t("MeetingStatus").as_deref(), Some("1"));
        assert_eq!(t("Sensitivity").as_deref(), Some("2"));
        assert_eq!(t("Location").as_deref(), Some("Room 1"));
        assert_eq!(t("ResponseType").as_deref(), Some("1"));
        let bytes = eas_wbxml::encode(&d);
        assert_eq!(eas_wbxml::decode(&bytes).unwrap(), d);
        // For someone else it's a received meeting.
        let other = e.to_element(&["bob@example.org".into()], true);
        assert_eq!(
            other.child_text(Page::Calendar, "MeetingStatus").as_deref(),
            Some("3")
        );
        assert_eq!(
            other.child_text(Page::Calendar, "ResponseType").as_deref(),
            Some("2")
        );
        assert!(other.child(Page::AirSyncBase, "Location").is_some());
    }

    #[test]
    fn device_echo_patches_nothing() {
        let stored = weekly_meeting();
        let current = EasEvent::from_jmap(&stored).unwrap();
        let data = current.to_element(&me(), false);
        let dev = EasEvent::from_device(&data, Some(&current), 2026).unwrap();
        assert_eq!(dev.tz, current.tz);
        let p = dev.patch(&current, &stored, &me());
        assert!(p.is_empty(), "{p:?}");
    }

    #[test]
    fn device_edits_patch_groups() {
        let stored = weekly_meeting();
        let current = EasEvent::from_jmap(&stored).unwrap();
        let mut dev = current.clone();
        dev.start += Duration::hours(1);
        dev.end += Duration::hours(1);
        dev.subject = Some("Standup v2".into());
        dev.reminder = None;
        dev.attendees.push(Attendee {
            email: "carol@example.org".into(),
            name: "Carol".into(),
            status: 0,
            kind: 2,
        });
        let p = dev.patch(&current, &stored, &me());
        assert_eq!(p["start"], json!("2026-10-19T10:30:00"));
        assert_eq!(p["title"], json!("Standup v2"));
        assert_eq!(p["alerts"], Value::Null);
        // Overrides are rewritten with the new times but keep unknown keys.
        assert_eq!(
            p["recurrenceOverrides"]["2026-10-26T09:30:00"]["x-unknown"],
            json!(1)
        );
        let parts = p["participants"].as_object().unwrap();
        assert_eq!(
            parts["p1"]["participationStatus"],
            json!("tentative"),
            "kept"
        );
        assert!(parts
            .values()
            .any(|x| x["calendarAddress"] == "mailto:carol@example.org"
                && x["roles"]["optional"] == true));
        assert!(!p.contains_key("description") && !p.contains_key("recurrenceRule"));
    }

    #[test]
    fn new_all_day_event_from_device() {
        // All-day on 24 Dec in Berlin: local midnight is 23:00 UTC the day before.
        let blob = tz::from_iana("Europe/Berlin".parse().unwrap(), 2026).encode();
        let data = Element::new(Page::AirSync, "ApplicationData")
            .with_child(Element::new(Page::Calendar, "TimeZone").with_text(blob))
            .with_child(Element::new(Page::Calendar, "AllDayEvent").with_text("1"))
            .with_child(Element::new(Page::Calendar, "StartTime").with_text("20261223T230000Z"))
            .with_child(Element::new(Page::Calendar, "EndTime").with_text("20261224T230000Z"))
            .with_child(Element::new(Page::Calendar, "Subject").with_text("Holiday"))
            .with_child(Element::new(Page::Calendar, "Reminder").with_text("0"))
            .with_child(
                Element::new(Page::Calendar, "Recurrence")
                    .with_child(Element::new(Page::Calendar, "Recurrence_Type").with_text("5"))
                    .with_child(
                        Element::new(Page::Calendar, "Recurrence_DayOfMonth").with_text("24"),
                    )
                    .with_child(
                        Element::new(Page::Calendar, "Recurrence_MonthOfYear").with_text("12"),
                    ),
            );
        let dev = EasEvent::from_device(&data, None, 2026).unwrap();
        let ev = dev.to_new_jmap("cal1", &me());
        assert_eq!(ev["start"], json!("2026-12-24T00:00:00"));
        assert_eq!(ev["duration"], json!("P1D"));
        assert_eq!(ev["timeZone"], Value::Null);
        assert_eq!(ev["showWithoutTime"], json!(true));
        assert_eq!(ev["recurrenceRule"]["frequency"], json!("yearly"));
        assert_eq!(ev["recurrenceRule"]["byMonth"], json!(["12"]));
        assert_eq!(ev["calendarIds"], json!({"cal1": true}));
        assert!(ev.get("participants").is_none());
        // And back: a floating all-day event goes out as UTC midnight.
        let back = EasEvent::from_jmap(&ev).unwrap();
        assert_eq!(eas_compact(back.start), "20261224T000000Z");
        assert!(back.all_day);
    }

    #[test]
    fn monthly_nth_rules_round_trip() {
        let tz: Tz = "America/New_York".parse().unwrap();
        let start = to_utc(tz, parse_local("2026-11-24T18:00:00").unwrap());
        let rule = json!({"frequency": "monthly", "byDay": [{"day": "tu", "nthOfPeriod": -1}], "count": 6});
        let r = rule_to_eas(&rule, tz, start);
        assert_eq!(
            (r.kind, r.day_of_week, r.week_of_month, r.occurrences),
            (3, 4, Some(5), Some(6))
        );
        let back = eas_to_rule(&r, tz);
        assert_eq!(back["byDay"][0]["nthOfPeriod"], json!(-1));
        assert_eq!(rule_to_eas(&back, tz, start), r);
    }
}
