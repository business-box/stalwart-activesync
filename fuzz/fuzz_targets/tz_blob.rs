//! Time zone blobs from devices, matched to IANA zones.
#![no_main]
use base64::Engine as _;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let blob = base64::engine::general_purpose::STANDARD.encode(data);
    if let Some(info) = eas_convert::tz::TimeZoneInfo::decode(&blob) {
        let _ = eas_convert::tz::to_iana(&info, 2026);
        let _ = eas_convert::tz::fixed_offset(&info);
        let _ = info.encode();
    }
});
