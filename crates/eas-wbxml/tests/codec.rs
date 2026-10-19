use eas_wbxml::{decode, decode_with_limits, encode, DecodeError, Element, Limits, Node, Page};

const FOLDER_SYNC_0: [u8; 13] = [
    0x03, 0x01, 0x6A, 0x00, // header
    0x00, 0x07, // SWITCH_PAGE FolderHierarchy
    0x56, // <FolderSync> with content
    0x52, // <SyncKey> with content
    0x03, b'0', 0x00, // "0"
    0x01, 0x01, // </SyncKey></FolderSync>
];

#[test]
fn folder_sync_request_vector() {
    let el = decode(&FOLDER_SYNC_0).unwrap();
    assert!(el.is(Page::FolderHierarchy, "FolderSync"));
    assert_eq!(
        el.child_text(Page::FolderHierarchy, "SyncKey").as_deref(),
        Some("0")
    );
    assert_eq!(encode(&el), FOLDER_SYNC_0);
}

fn sample_sync_response() -> Element {
    let app_data = Element::new(Page::AirSync, "ApplicationData")
        .with_child(Element::new(Page::Email, "Subject").with_text("Grüße & <Hallo>"))
        .with_child(Element::new(Page::Email, "Read").with_text("0"))
        .with_child(
            Element::new(Page::AirSyncBase, "Body")
                .with_text_child("Type", "4")
                .with_child(
                    Element::new(Page::AirSyncBase, "Data").with_opaque(vec![0u8, 1, 2, 255]),
                ),
        )
        .with_child(Element::new(Page::Email2, "ConversationId").with_opaque(vec![7u8; 16]));
    let add = Element::new(Page::AirSync, "Add")
        .with_text_child("ServerId", "m:42")
        .with_child(app_data);
    let collection = Element::new(Page::AirSync, "Collection")
        .with_text_child("SyncKey", "7")
        .with_text_child("CollectionId", "1")
        .with_text_child("Status", "1")
        .with_child(Element::new(Page::AirSync, "Commands").with_child(add))
        .with_child(Element::new(Page::AirSync, "MoreAvailable"));
    Element::new(Page::AirSync, "Sync")
        .with_child(Element::new(Page::AirSync, "Collections").with_child(collection))
}

#[test]
fn round_trip_with_page_switches_and_opaque() {
    let doc = sample_sync_response();
    let bytes = encode(&doc);
    assert_eq!(decode(&bytes).unwrap(), doc);
}

#[test]
fn empty_element_has_no_content_flag() {
    let bytes = encode(&sample_sync_response());
    // <MoreAvailable/> is AirSync 0x14 without the 0x40 content bit.
    assert!(bytes.contains(&0x14));
}

#[test]
fn xml_rendering_hides_opaque_and_escapes() {
    let xml = sample_sync_response().to_xml();
    assert!(xml.contains("<Sync xmlns=\"AirSync:\">"));
    assert!(xml.contains("<Subject xmlns=\"Email:\">Grüße &amp; &lt;Hallo&gt;</Subject>"));
    assert!(xml.contains("[opaque 4 bytes]"));
    assert!(!xml.contains('\u{0}'));
}

#[test]
fn accessors() {
    let doc = sample_sync_response();
    let coll = doc
        .child(Page::AirSync, "Collections")
        .and_then(|c| c.child(Page::AirSync, "Collection"))
        .unwrap();
    assert_eq!(
        coll.child_text(Page::AirSync, "SyncKey").as_deref(),
        Some("7")
    );
    let data = coll
        .child(Page::AirSync, "Commands")
        .and_then(|c| c.child(Page::AirSync, "Add"))
        .and_then(|a| a.child(Page::AirSync, "ApplicationData"))
        .and_then(|a| a.child(Page::AirSyncBase, "Body"))
        .and_then(|b| b.child(Page::AirSyncBase, "Data"))
        .unwrap();
    assert_eq!(data.bytes().unwrap(), vec![0, 1, 2, 255]);
    assert!(matches!(data.children[0], Node::Opaque(_)));
}

#[test]
fn rejects_malformed_input() {
    assert_eq!(decode(&[]), Err(DecodeError::Eof(0)));
    assert_eq!(
        decode(&[0x03, 0x01, 0x04, 0x00, 0x45]),
        Err(DecodeError::Charset(4))
    );
    // Truncated: FolderSync opened, never closed.
    assert_eq!(decode(&FOLDER_SYNC_0[..11]), Err(DecodeError::Unclosed(2)));
    // Unknown page 30.
    assert!(matches!(
        decode(&[0x03, 0x01, 0x6A, 0x00, 0x00, 30, 0x45]),
        Err(DecodeError::UnknownPage { page: 30, .. })
    ));
    // Attributes are not used by EAS.
    assert!(matches!(
        decode(&[0x03, 0x01, 0x6A, 0x00, 0xC5, 0x01]),
        Err(DecodeError::Attributes(4))
    ));
    // Opaque claims more bytes than exist.
    assert!(matches!(
        decode(&[0x03, 0x01, 0x6A, 0x00, 0x45, 0xC3, 0x81, 0x00, 1, 2]),
        Err(DecodeError::OpaqueTooLong { len: 128, .. })
    ));
    // Unterminated string.
    assert!(matches!(
        decode(&[0x03, 0x01, 0x6A, 0x00, 0x45, 0x03, b'a']),
        Err(DecodeError::UnterminatedString(_))
    ));
    // Bytes after the root.
    let mut trailing = FOLDER_SYNC_0.to_vec();
    trailing.push(0x45);
    assert!(matches!(decode(&trailing), Err(DecodeError::Trailing(_))));
    // Header only.
    assert_eq!(decode(&[0x03, 0x01, 0x6A, 0x00]), Err(DecodeError::Empty));
}

#[test]
fn enforces_limits() {
    let mut deep = vec![0x03, 0x01, 0x6A, 0x00];
    deep.extend(std::iter::repeat_n(0x45u8, 100));
    deep.extend(std::iter::repeat_n(0x01u8, 100));
    let limits = Limits {
        max_depth: 10,
        ..Limits::default()
    };
    assert_eq!(
        decode_with_limits(&deep, &limits),
        Err(DecodeError::TooDeep(10))
    );
    let limits = Limits {
        max_nodes: 5,
        ..Limits::default()
    };
    assert_eq!(
        decode_with_limits(&deep, &limits),
        Err(DecodeError::TooManyNodes(5))
    );
}

/// Cheap stand-in for cargo-fuzz in CI: random and mutated inputs must never
/// panic, and whatever decodes must re-encode to something that decodes the
/// same.
#[test]
fn never_panics_on_garbage() {
    let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let base = encode(&sample_sync_response());
    for i in 0..20_000 {
        let mut input = if i % 2 == 0 {
            let len = (next() % 64) as usize;
            let mut v = vec![0x03, 0x01, 0x6A, 0x00];
            v.extend((0..len).map(|_| next() as u8));
            v
        } else {
            base.clone()
        };
        for _ in 0..(next() % 4) {
            let idx = (next() as usize) % input.len();
            input[idx] = next() as u8;
        }
        if let Ok(el) = decode(&input) {
            assert_eq!(decode(&encode(&el)).unwrap(), el);
        }
    }
}
