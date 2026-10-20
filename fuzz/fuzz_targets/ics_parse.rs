//! Invitation calendar parts from mail.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Some(inv) = eas_convert::ics::parse(&String::from_utf8_lossy(data)) {
        let _ = inv.message_class();
        if let Some(ev) = inv.to_event() {
            let _ = eas_convert::email::meeting_request(&ev, inv.recurring, true);
        }
    }
});
