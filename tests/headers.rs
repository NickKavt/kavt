//! The headers every answer carries: the interface must not be framed by another site, and a
//! browser must not guess a file's type.
#[test]
fn security_headers_are_declared_once() {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/main.rs")).unwrap();
    for name in ["CONTENT_SECURITY_POLICY", "X_FRAME_OPTIONS", "X_CONTENT_TYPE_OPTIONS", "REFERRER_POLICY"] {
        assert_eq!(src.matches(&format!("header::{name}")).count(), 1, "{name} is set once");
    }
    assert!(src.contains("frame-ancestors 'none'"), "the interface refuses to be framed");
}
