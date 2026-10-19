//! Windows `TIME_ZONE_INFORMATION` blobs (MS-ASDTYPE 2.6.4), which EAS
//! uses for calendar time zones, to and from IANA zones.
//!
//! A blob holds a bias and the yearly rules for entering and leaving
//! daylight time, not a zone name. Going out, the rules are derived from
//! the IANA zone for the current year. Coming in, the blob is matched
//! against the rules of a list of common zones; any zone with the same
//! rules gives the same times, so the first match is good enough.

use base64::Engine as _;
use chrono::{
    DateTime, Datelike, Duration, NaiveDate, NaiveDateTime, Offset, TimeZone, Timelike, Utc,
};
use chrono_tz::{OffsetComponents, Tz};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// When daylight time starts or ends: month, weekday (0 = Sunday), week
/// of the month (5 = last) and local hour and minute.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rule {
    pub month: u16,
    pub day_of_week: u16,
    pub week: u16,
    pub hour: u16,
    pub minute: u16,
}

/// The parts of `TIME_ZONE_INFORMATION` that decide times.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TimeZoneInfo {
    /// Minutes to add to local standard time to get UTC.
    pub bias: i32,
    pub standard_name: String,
    pub standard: Rule,
    pub standard_bias: i32,
    pub daylight_name: String,
    pub daylight: Rule,
    pub daylight_bias: i32,
}

impl TimeZoneInfo {
    fn has_dst(&self) -> bool {
        self.daylight.month != 0 && self.standard.month != 0
    }

    /// Same times as `other`, ignoring names.
    pub fn same_rules(&self, other: &TimeZoneInfo) -> bool {
        self.bias == other.bias
            && (!self.has_dst() && !other.has_dst()
                || self.standard == other.standard
                    && self.daylight == other.daylight
                    && self.daylight_bias == other.daylight_bias
                    && self.standard_bias == other.standard_bias)
    }

    pub fn encode(&self) -> String {
        let mut b = Vec::with_capacity(172);
        b.extend_from_slice(&self.bias.to_le_bytes());
        push_name(&mut b, &self.standard_name);
        push_rule(&mut b, &self.standard);
        b.extend_from_slice(&self.standard_bias.to_le_bytes());
        push_name(&mut b, &self.daylight_name);
        push_rule(&mut b, &self.daylight);
        b.extend_from_slice(&self.daylight_bias.to_le_bytes());
        base64::engine::general_purpose::STANDARD.encode(b)
    }

    pub fn decode(blob: &str) -> Option<TimeZoneInfo> {
        let b = base64::engine::general_purpose::STANDARD
            .decode(blob.trim())
            .ok()?;
        if b.len() < 172 {
            return None;
        }
        let i32_at = |o: usize| i32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
        let u16_at = |o: usize| u16::from_le_bytes([b[o], b[o + 1]]);
        let name_at = |o: usize| {
            let units: Vec<u16> = (0..32)
                .map(|i| u16_at(o + i * 2))
                .take_while(|u| *u != 0)
                .collect();
            String::from_utf16_lossy(&units)
        };
        // SYSTEMTIME: year, month, day of week, day, hour, minute, ...
        let rule_at = |o: usize| Rule {
            month: u16_at(o + 2),
            day_of_week: u16_at(o + 4),
            week: u16_at(o + 6),
            hour: u16_at(o + 8),
            minute: u16_at(o + 10),
        };
        Some(TimeZoneInfo {
            bias: i32_at(0),
            standard_name: name_at(4),
            standard: rule_at(68),
            standard_bias: i32_at(84),
            daylight_name: name_at(88),
            daylight: rule_at(152),
            daylight_bias: i32_at(168),
        })
    }
}

fn push_name(b: &mut Vec<u8>, name: &str) {
    let units: Vec<u16> = name.encode_utf16().take(31).collect();
    for i in 0..32 {
        b.extend_from_slice(&units.get(i).copied().unwrap_or(0).to_le_bytes());
    }
}

fn push_rule(b: &mut Vec<u8>, r: &Rule) {
    for v in [0, r.month, r.day_of_week, r.week, r.hour, r.minute, 0, 0] {
        b.extend_from_slice(&v.to_le_bytes());
    }
}

/// The rule for a transition that happens at `local` wall-clock time.
fn rule_for(local: NaiveDateTime) -> Rule {
    let d = local.date();
    let next_month = if d.month() == 12 {
        NaiveDate::from_ymd_opt(d.year() + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(d.year(), d.month() + 1, 1)
    };
    let days_in_month = next_month
        .and_then(|n| n.pred_opt())
        .map(|l| l.day())
        .unwrap_or(31);
    let week = if d.day() + 7 > days_in_month {
        5
    } else {
        ((d.day() - 1) / 7 + 1) as u16
    };
    Rule {
        month: d.month() as u16,
        day_of_week: d.weekday().num_days_from_sunday() as u16,
        week,
        hour: local.hour() as u16,
        minute: local.minute() as u16,
    }
}

fn offset_minutes(tz: Tz, at: DateTime<Utc>) -> (i32, i32) {
    let o = tz.offset_from_utc_datetime(&at.naive_utc());
    (
        o.base_utc_offset().num_minutes() as i32,
        o.dst_offset().num_minutes() as i32,
    )
}

/// The blob for an IANA zone, with the rules of `year`.
pub fn from_iana(tz: Tz, year: i32) -> TimeZoneInfo {
    let start = Utc.with_ymd_and_hms(year, 1, 1, 0, 0, 0).unwrap();
    let (base, _) = offset_minutes(tz, start);
    let mut info = TimeZoneInfo {
        bias: -base,
        standard_name: tz.name().to_string(),
        daylight_name: tz.name().to_string(),
        ..Default::default()
    };
    // Find the instants in the year where the DST part changes: by day,
    // then by quarter hour within the day.
    let mut prev = offset_minutes(tz, start).1;
    let mut day = start;
    for _ in 0..366 {
        let next = day + Duration::days(1);
        let dst = offset_minutes(tz, next).1;
        if dst != prev {
            let mut t = day;
            while offset_minutes(tz, t + Duration::minutes(15)).1 == prev {
                t += Duration::minutes(15);
            }
            let at = t + Duration::minutes(15);
            // Wall-clock time just before the change, in the old offset.
            let local = (at + Duration::minutes((base + prev) as i64)).naive_utc();
            if dst > prev {
                info.daylight = rule_for(local);
                info.daylight_bias = -dst;
            } else {
                info.standard = rule_for(local);
            }
            prev = dst;
        }
        day = next;
    }
    if !info.has_dst() {
        info.standard = Rule::default();
        info.daylight = Rule::default();
        info.daylight_bias = 0;
    }
    info
}

/// Zones tried when matching a blob, most used first.
const CANDIDATES: &[&str] = &[
    "Europe/Berlin",
    "Europe/London",
    "Europe/Helsinki",
    "Europe/Moscow",
    "Europe/Istanbul",
    "Europe/Lisbon",
    "Atlantic/Reykjavik",
    "Atlantic/Azores",
    "Africa/Cairo",
    "Africa/Johannesburg",
    "Africa/Lagos",
    "Africa/Nairobi",
    "Africa/Casablanca",
    "America/New_York",
    "America/Chicago",
    "America/Denver",
    "America/Phoenix",
    "America/Los_Angeles",
    "America/Anchorage",
    "Pacific/Honolulu",
    "America/Halifax",
    "America/St_Johns",
    "America/Sao_Paulo",
    "America/Argentina/Buenos_Aires",
    "America/Bogota",
    "America/Mexico_City",
    "America/Santiago",
    "America/Caracas",
    "Asia/Dubai",
    "Asia/Tehran",
    "Asia/Karachi",
    "Asia/Kolkata",
    "Asia/Kathmandu",
    "Asia/Dhaka",
    "Asia/Bangkok",
    "Asia/Shanghai",
    "Asia/Singapore",
    "Asia/Tokyo",
    "Asia/Seoul",
    "Asia/Jerusalem",
    "Asia/Riyadh",
    "Asia/Yangon",
    "Australia/Perth",
    "Australia/Adelaide",
    "Australia/Darwin",
    "Australia/Brisbane",
    "Australia/Sydney",
    "Australia/Hobart",
    "Pacific/Auckland",
    "Pacific/Fiji",
    "Pacific/Tongatapu",
    "Pacific/Kiritimati",
    "Pacific/Chatham",
    "UTC",
];

fn cache() -> &'static Mutex<HashMap<(String, i32), TimeZoneInfo>> {
    static C: OnceLock<Mutex<HashMap<(String, i32), TimeZoneInfo>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// [`from_iana`], cached per zone and year.
pub fn info_for(tz: Tz, year: i32) -> TimeZoneInfo {
    let key = (tz.name().to_string(), year);
    if let Some(i) = cache().lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return i.clone();
    }
    let info = from_iana(tz, year);
    cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key, info.clone());
    info
}

/// An IANA zone with the blob's rules in `year`. The blob's own name wins
/// when it is an IANA name with the same rules (what this gateway sends).
pub fn to_iana(info: &TimeZoneInfo, year: i32) -> Option<Tz> {
    if let Ok(tz) = info.standard_name.parse::<Tz>() {
        if info_for(tz, year).same_rules(info) {
            return Some(tz);
        }
    }
    for name in CANDIDATES {
        let tz: Tz = name.parse().ok()?;
        if info_for(tz, year).same_rules(info) {
            return Some(tz);
        }
    }
    // A fixed offset in whole hours has an Etc zone (with inverted sign).
    if !info.has_dst() && info.bias % 60 == 0 {
        let h = info.bias / 60;
        let name = match h {
            0 => "Etc/UTC".to_string(),
            h if h > 0 => format!("Etc/GMT+{h}"),
            h => format!("Etc/GMT{h}"),
        };
        return name.parse().ok();
    }
    None
}

/// Fixed offset from UTC in minutes for a blob, ignoring daylight time;
/// the last resort when no zone matches.
pub fn fixed_offset(info: &TimeZoneInfo) -> chrono::FixedOffset {
    chrono::FixedOffset::east_opt(-info.bias * 60).unwrap_or(Utc.fix())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn berlin_rules() {
        let tz: Tz = "Europe/Berlin".parse().unwrap();
        let i = from_iana(tz, 2026);
        assert_eq!(i.bias, -60);
        assert_eq!(i.daylight_bias, -60);
        // Last Sunday in March, 02:00; last Sunday in October, 03:00.
        assert_eq!(
            i.daylight,
            Rule {
                month: 3,
                day_of_week: 0,
                week: 5,
                hour: 2,
                minute: 0
            }
        );
        assert_eq!(
            i.standard,
            Rule {
                month: 10,
                day_of_week: 0,
                week: 5,
                hour: 3,
                minute: 0
            }
        );
        let back = TimeZoneInfo::decode(&i.encode()).unwrap();
        assert_eq!(back, i);
        assert_eq!(to_iana(&back, 2026), Some(tz));
    }

    #[test]
    fn new_york_and_southern_hemisphere() {
        let ny: Tz = "America/New_York".parse().unwrap();
        let i = from_iana(ny, 2026);
        assert_eq!(i.bias, 300);
        // Second Sunday in March, first Sunday in November.
        assert_eq!(
            (i.daylight.month, i.daylight.week, i.daylight.hour),
            (3, 2, 2)
        );
        assert_eq!(
            (i.standard.month, i.standard.week, i.standard.hour),
            (11, 1, 2)
        );

        let syd: Tz = "Australia/Sydney".parse().unwrap();
        let s = from_iana(syd, 2026);
        assert_eq!(s.bias, -600);
        assert_eq!(s.daylight.month, 10);
        assert_eq!(s.standard.month, 4);
    }

    #[test]
    fn no_dst_and_matching_foreign_blobs() {
        let tokyo: Tz = "Asia/Tokyo".parse().unwrap();
        let i = from_iana(tokyo, 2026);
        assert_eq!(i.bias, -540);
        assert_eq!(i.daylight, Rule::default());
        // A blob from a device names the zone its own way; the rules match.
        let mut foreign = from_iana("Europe/Paris".parse().unwrap(), 2026);
        foreign.standard_name = "W. Europe Standard Time".into();
        foreign.daylight_name = "W. Europe Daylight Time".into();
        let tz = to_iana(&TimeZoneInfo::decode(&foreign.encode()).unwrap(), 2026).unwrap();
        assert_eq!(tz.name(), "Europe/Berlin");
        // Unknown fixed offset in hours: an Etc zone.
        let odd = TimeZoneInfo {
            bias: -180,
            ..Default::default()
        };
        let tz = to_iana(&odd, 2026).unwrap();
        let now = Utc::now();
        assert_eq!(
            tz.offset_from_utc_datetime(&now.naive_utc())
                .fix()
                .local_minus_utc(),
            3 * 3600
        );
        assert!(TimeZoneInfo::decode("short").is_none());
    }
}
