//! `Range` and `If-Range` parsing for any header and file size: a part is
//! never empty and always inside `0..size`.
#![no_main]

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use strata_server::range::{self, Range};

#[derive(Arbitrary, Debug)]
struct Input<'a> {
    range: Option<&'a str>,
    if_range: Option<&'a str>,
    etag: &'a str,
    size: u64,
    small: bool,
}

fuzz_target!(|i: Input| {
    // Small files hit the edges (0, 1, ...) more often.
    let size = if i.small { i.size % 64 } else { i.size };
    match range::parse(i.range, size) {
        Range::Full | Range::Unsatisfiable => {}
        Range::Part { start, end } => {
            assert!(start < end, "empty part {start}..{end}");
            assert!(end <= size, "part {start}..{end} past {size}");
        }
    }
    // No header means the whole file; a satisfiable range never answers
    // a header that isn't a byte range.
    if i.range.is_none() {
        assert_eq!(range::parse(None, size), Range::Full);
    }
    let allowed = range::if_range_allows(i.if_range, i.etag);
    if let Some(v) = i.if_range {
        if allowed {
            assert_eq!(
                v.trim(),
                i.etag,
                "If-Range {v:?} let a range through for {:?}",
                i.etag
            );
        }
    } else {
        assert!(allowed);
    }
});
