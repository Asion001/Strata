//! `Content-Disposition` for any file name: never a CR, LF or NUL, and the
//! quoted ASCII name has no quote or backslash that could end it early.
#![no_main]

use libfuzzer_sys::fuzz_target;
use strata_server::safe::content_disposition;

fuzz_target!(|data: (bool, &str)| {
    let (inline, name) = data;
    let v = content_disposition(inline, name);
    let b = v.as_bytes();
    assert!(!b.iter().any(|c| matches!(c, b'\r' | b'\n' | 0)), "{v:?}");
    if b == b"attachment" {
        return; // the fallback
    }
    let s = v.to_str().expect("an ASCII header");
    let kind = if inline { "inline" } else { "attachment" };
    let rest = s
        .strip_prefix(&format!("{kind}; filename=\""))
        .expect("starts with the kind");
    // The quoted name ends at the first quote, and must be followed by the
    // UTF-8 form, which is plain RFC 5987 attr-chars and %XX.
    let end = rest.find('"').expect("closing quote");
    let quoted = &rest[..end];
    assert!(!quoted.contains('\\'), "{v:?}");
    assert!(!quoted.is_empty());
    let star = rest[end..]
        .strip_prefix("\"; filename*=UTF-8''")
        .expect("filename* after the quoted name");
    assert!(
        star.bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"!#$&+-.^_`|~%".contains(&c)),
        "{v:?}"
    );
});
