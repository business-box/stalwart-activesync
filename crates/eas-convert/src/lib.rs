//! Conversions between JMAP objects and EAS data classes (phases 2 to 4):
//! email serialisation and MIME rebuilding for SmartReply/SmartForward,
//! JSCalendar to EAS calendar items including recurrence and Windows
//! `TIME_ZONE_INFORMATION` blobs, and JSContact to EAS contacts with
//! patch-only updates.
