//! WBXML from a device turned into calendar events, contact patches and
//! email changes, as the Sync handler does with ApplicationData.
#![no_main]
use eas_convert::calendar::EasEvent;
use eas_convert::{contact, email};
use libfuzzer_sys::fuzz_target;
use serde_json::json;

fuzz_target!(|data: &[u8]| {
    let Ok(el) = eas_wbxml::decode(data) else {
        return;
    };
    let me = vec!["alice@example.org".to_string()];
    let stored = json!({
        "start": "2026-10-19T09:30:00", "timeZone": "Europe/Berlin", "duration": "PT1H",
        "title": "x", "recurrenceRule": {"frequency": "weekly"},
        "participants": {"p": {"calendarAddress": "mailto:bob@example.org"}}
    });
    let base = EasEvent::from_jmap(&stored);
    if let Some(ev) = EasEvent::from_device(&el, base.as_ref(), 2026) {
        let _ = ev.to_new_jmap("cal", &me);
        let _ = ev.to_element(&me, true);
        if let Some(b) = &base {
            let _ = ev.patch(b, &stored, &me);
        }
    }
    let card = json!({"name": {"components": [{"kind": "given", "value": "A"}]}, "emails": {"e": {"address": "a@b"}}});
    let _ = contact::device_patch(&card, &el, None);
    let _ = contact::new_card(&el, "ab");
    let _ = email::change_patch(&el);
});
