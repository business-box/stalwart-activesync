//! Conversions between JMAP objects and EAS data classes.
//!
//! Implemented: email items ([`email`]) with body selection, truncation and
//! attachments, and device-side read/flag changes back to JMAP patches.
//! Still to come: MIME rebuilding for SmartReply/SmartForward, JSCalendar to
//! EAS calendar items including recurrence and Windows
//! `TIME_ZONE_INFORMATION` blobs, and JSContact to EAS contacts with
//! patch-only updates.

pub mod compose;
pub mod email;
pub mod text;
