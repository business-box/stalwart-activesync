//! Outgoing MIME from devices: envelope, Bcc removal, smart reply rebuild.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(out) = eas_convert::compose::prepare(data) {
        let _ = eas_convert::compose::prepare(&out.mime);
    }
    let original = eas_convert::compose::Original {
        text: "original".into(),
        message_id: Some("id@x".into()),
        ..Default::default()
    };
    if let Some(m) = eas_convert::compose::smart(data, &original, false) {
        let _ = eas_convert::compose::prepare(&m);
    }
});
