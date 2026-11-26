//! `paths::normalize` on any user path: never escapes the root, and is
//! idempotent. Also `join`, `split` and `strip_prefix` on what it returns.
#![no_main]

use std::path::{Component, Path};

use libfuzzer_sys::fuzz_target;
use strata_storage::paths;

fuzz_target!(|data: &[u8]| {
    // Request paths reach the server as UTF-8 (anything else is refused
    // while decoding); invalid bytes are fuzzed through a lossy decode.
    let input = match std::str::from_utf8(data) {
        Ok(s) => s.to_string(),
        Err(_) => String::from_utf8_lossy(data).into_owned(),
    };
    let Ok(p) = paths::normalize(&input) else {
        return;
    };
    // Idempotent.
    assert_eq!(
        paths::normalize(&p).as_deref(),
        Ok(p.as_str()),
        "not idempotent"
    );
    // Relative, no empty, "." or ".." component, no control character.
    assert!(!p.starts_with('/') && !p.ends_with('/'), "{p:?}");
    if !p.is_empty() {
        for part in p.split('/') {
            assert!(!part.is_empty() && part != "." && part != "..", "{p:?}");
            assert!(paths::validate_name(part).is_ok(), "{part:?}");
            assert!(!part.chars().any(char::is_control), "{part:?}");
        }
    }
    // Below a base folder, it stays below it.
    let base = Path::new("/srv/base");
    let full = base.join(&p);
    assert!(full.starts_with(base), "{p:?} escapes");
    assert!(
        full.components()
            .skip(base.components().count())
            .all(|c| matches!(c, Component::Normal(_))),
        "{p:?} has a non-plain component"
    );
    // split / join round-trip, strip_prefix of itself.
    if let Some((parent, name)) = paths::split(&p) {
        assert_eq!(paths::join(parent, name), p);
        assert_eq!(paths::strip_prefix(&p, parent).is_some(), true);
    }
    assert_eq!(paths::strip_prefix(&p, &p), Some(""));
});
