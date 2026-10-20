//! Query strings in both forms, including the binary base64 one.
#![no_main]
use base64::Engine as _;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    if let Ok(line) = eas_proto::RequestLine::parse(&text) {
        let _ = line.command();
    }
    let b64 = base64::engine::general_purpose::STANDARD.encode(data);
    if let Ok(line) = eas_proto::RequestLine::parse(&b64) {
        let _ = line.command();
    }
});
