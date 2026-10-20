//! Any bytes: decoding never panics, and what decodes survives a round trip.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(el) = eas_wbxml::decode(data) {
        let bytes = eas_wbxml::encode(&el);
        let again = eas_wbxml::decode(&bytes).expect("re-decoding our own encoding");
        assert_eq!(again, el);
        let _ = el.to_xml();
    }
});
