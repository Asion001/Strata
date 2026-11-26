//! HTTP byte ranges (RFC 9110 §14): one range per response; anything else
//! (no header, another unit, several ranges, a malformed one) gets the whole
//! file, as the RFC allows.

/// What to send for a request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Range {
    /// The whole file (200).
    Full,
    /// Bytes `start..end` (206); never empty.
    Part { start: u64, end: u64 },
    /// A well-formed range that misses the file (416).
    Unsatisfiable,
}

/// A run of ASCII digits as a number; a run too long for u64 saturates (it
/// is past the end of any file either way). None if not all digits.
fn number(s: &str) -> Option<u64> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(s.parse::<u64>().unwrap_or(u64::MAX))
}

/// Interpret a `Range` header value for a file of `size` bytes.
pub fn parse(header: Option<&str>, size: u64) -> Range {
    let Some(v) = header else {
        return Range::Full;
    };
    let v = v.trim();
    let Some((unit, spec)) = v.split_once('=') else {
        return Range::Full;
    };
    if !unit.trim().eq_ignore_ascii_case("bytes") {
        return Range::Full;
    }
    let spec = spec.trim();
    if spec.contains(',') {
        return Range::Full; // several ranges: the whole file instead
    }
    let Some((a, b)) = spec.split_once('-') else {
        return Range::Full;
    };
    let (a, b) = (a.trim(), b.trim());
    match (a.is_empty(), b.is_empty()) {
        // bytes=-n: the last n bytes
        (true, false) => match number(b) {
            None => Range::Full,
            Some(0) => Range::Unsatisfiable,
            // satisfiable (RFC 9110 §14.1.1), but there is nothing to slice
            Some(_) if size == 0 => Range::Full,
            Some(n) => Range::Part {
                start: size - n.min(size),
                end: size,
            },
        },
        // bytes=a-
        (false, true) => match number(a) {
            None => Range::Full,
            Some(s) if s >= size => Range::Unsatisfiable,
            Some(s) => Range::Part {
                start: s,
                end: size,
            },
        },
        // bytes=a-b (inclusive)
        (false, false) => match (number(a), number(b)) {
            (Some(s), Some(e)) if e >= s => {
                if s >= size {
                    Range::Unsatisfiable
                } else {
                    // e.min(size - 1) + 1 can't overflow: size - 1 < u64::MAX
                    Range::Part {
                        start: s,
                        end: e.min(size - 1) + 1,
                    }
                }
            }
            // last < first is invalid syntax: ignored
            _ => Range::Full,
        },
        (true, true) => Range::Full,
    }
}

/// Does an `If-Range` precondition let a range through? Only an exact match
/// with the current ETag does; a weak tag or a date (we send no
/// Last-Modified) means the whole file.
pub fn if_range_allows(if_range: Option<&str>, etag: &str) -> bool {
    match if_range.map(str::trim) {
        None => true,
        Some(v) if v.starts_with('"') => v == etag,
        Some(_) => false,
    }
}

/// The chunks of a hashed file that cover bytes `start..end`, given the
/// chunks' sizes in order: (chunk index, byte range inside that chunk).
/// Only these chunks are read; the first and last are sliced.
pub fn covering(sizes: &[u64], start: u64, end: u64) -> Vec<(usize, std::ops::Range<u64>)> {
    let mut out = Vec::new();
    let mut off = 0u64;
    for (i, &sz) in sizes.iter().enumerate() {
        let (cs, ce) = (off, off.saturating_add(sz));
        off = ce;
        if cs >= end {
            break;
        }
        if ce <= start || sz == 0 {
            continue;
        }
        let a = start.saturating_sub(cs);
        let b = end.min(ce) - cs;
        out.push((i, a..b));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use Range::*;

    #[test]
    fn single_ranges() {
        let p = |h: &str| parse(Some(h), 100);
        assert_eq!(p("bytes=0-0"), Part { start: 0, end: 1 });
        assert_eq!(p("bytes=0-99"), Part { start: 0, end: 100 });
        assert_eq!(
            p("bytes=10-"),
            Part {
                start: 10,
                end: 100
            }
        );
        assert_eq!(
            p("bytes=-10"),
            Part {
                start: 90,
                end: 100
            }
        );
        // past the end: clamped
        assert_eq!(
            p("bytes=90-500"),
            Part {
                start: 90,
                end: 100
            }
        );
        assert_eq!(p("bytes=-500"), Part { start: 0, end: 100 });
        // tolerant of case and spaces
        assert_eq!(p(" Bytes = 1 - 2 "), Part { start: 1, end: 3 });
    }

    #[test]
    fn unsatisfiable() {
        let p = |h: &str| parse(Some(h), 100);
        assert_eq!(p("bytes=100-"), Unsatisfiable);
        assert_eq!(p("bytes=100-200"), Unsatisfiable);
        assert_eq!(p("bytes=-0"), Unsatisfiable);
        assert_eq!(parse(Some("bytes=0-"), 0), Unsatisfiable);
        assert_eq!(parse(Some("bytes=-0"), 0), Unsatisfiable);
        // a non-zero suffix of an empty file: the (empty) file
        assert_eq!(parse(Some("bytes=-5"), 0), Full);
    }

    #[test]
    fn ignored_means_whole_file() {
        let p = |h: &str| parse(Some(h), 100);
        assert_eq!(parse(None, 100), Full);
        for h in [
            "bytes=0-1,5-6",
            "items=0-1",
            "bytes=",
            "bytes=-",
            "bytes=5-3",
            "bytes=a-b",
            "bytes=+1-2",
            "bytes=1-+2",
            "bytes=0x1-2",
            "nonsense",
        ] {
            assert_eq!(p(h), Full, "{h}");
        }
    }

    #[test]
    fn huge_numbers_never_overflow() {
        let max = u64::MAX.to_string();
        assert_eq!(
            parse(Some(&format!("bytes=0-{max}")), 100),
            Part { start: 0, end: 100 }
        );
        assert_eq!(
            parse(Some("bytes=0-99999999999999999999999"), 100),
            Part { start: 0, end: 100 }
        );
        assert_eq!(parse(Some(&format!("bytes={max}-")), 100), Unsatisfiable);
        assert_eq!(
            parse(Some(&format!("bytes=-{max}")), 100),
            Part { start: 0, end: 100 }
        );
        assert_eq!(
            parse(Some(&format!("bytes=0-{max}")), u64::MAX),
            Part {
                start: 0,
                end: u64::MAX
            }
        );
        assert_eq!(
            parse(Some(&format!("bytes={}-", u64::MAX - 1)), u64::MAX),
            Part {
                start: u64::MAX - 1,
                end: u64::MAX
            }
        );
    }

    #[test]
    fn if_range() {
        let et = "\"r3-abc\"";
        assert!(if_range_allows(None, et));
        assert!(if_range_allows(Some(et), et));
        assert!(!if_range_allows(Some("\"r3-old\""), et));
        assert!(!if_range_allows(Some("W/\"r3-abc\""), et));
        assert!(!if_range_allows(Some("Tue, 15 Nov 1994 08:12:31 GMT"), et));
        assert!(!if_range_allows(Some(""), et)); // unreadable header
    }

    #[test]
    fn chunk_cover_at_boundaries() {
        let sizes = [10u64, 10, 10];
        assert_eq!(
            covering(&sizes, 0, 30),
            vec![(0, 0..10), (1, 0..10), (2, 0..10)]
        );
        // exactly one chunk
        assert_eq!(covering(&sizes, 10, 20), vec![(1, 0..10)]);
        // last byte of one chunk, first of the next
        assert_eq!(covering(&sizes, 9, 11), vec![(0, 9..10), (1, 0..1)]);
        // a single byte at a boundary
        assert_eq!(covering(&sizes, 10, 11), vec![(1, 0..1)]);
        assert_eq!(covering(&sizes, 29, 30), vec![(2, 9..10)]);
        assert!(covering(&sizes, 30, 30).is_empty());
        // empty chunks never appear
        assert_eq!(covering(&[0, 5, 0, 5], 3, 7), vec![(1, 3..5), (3, 0..2)]);
    }

    #[test]
    fn chunk_cover_matches_a_slice_for_every_range() {
        // Property: for random chunkings and every range, the covering
        // slices concatenated equal the original slice.
        use rand::{Rng, SeedableRng};
        let mut rng = rand::rngs::StdRng::seed_from_u64(7);
        for _ in 0..200 {
            let n = rng.gen_range(1..12);
            let sizes: Vec<u64> = (0..n).map(|_| rng.gen_range(0..9)).collect();
            let total: u64 = sizes.iter().sum();
            let data: Vec<u8> = (0..total).map(|i| i as u8).collect();
            let chunks: Vec<&[u8]> = {
                let mut off = 0usize;
                sizes
                    .iter()
                    .map(|&s| {
                        let c = &data[off..off + s as usize];
                        off += s as usize;
                        c
                    })
                    .collect()
            };
            for start in 0..=total {
                for end in start..=total {
                    let got: Vec<u8> = covering(&sizes, start, end)
                        .into_iter()
                        .flat_map(|(i, r)| chunks[i][r.start as usize..r.end as usize].to_vec())
                        .collect();
                    assert_eq!(
                        got,
                        &data[start as usize..end as usize],
                        "{sizes:?} {start}..{end}"
                    );
                }
            }
        }
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig {
            cases: std::env::var("PROPTEST_CASES").ok().and_then(|c| c.parse().ok()).unwrap_or(256),
            ..Default::default()
        })]

        /// For any chunking and any `bytes=a-b` header, the covering chunk
        /// slices concatenated are exactly the requested bytes.
        #[test]
        fn a_range_maps_to_exactly_its_bytes(
            sizes in proptest::collection::vec(0u64..64, 0..12),
            a in 0u64..800,
            b in 0u64..800,
            suffix in proptest::bool::ANY,
        ) {
            let total: u64 = sizes.iter().sum();
            let data: Vec<u8> = (0..total).map(|i| (i * 7 + 3) as u8).collect();
            let header = if suffix { format!("bytes=-{a}") } else { format!("bytes={a}-{b}") };
            match parse(Some(&header), total) {
                Part { start, end } => {
                    proptest::prop_assert!(start < end && end <= total);
                    let mut off = 0usize;
                    let starts: Vec<usize> = sizes.iter().map(|&s| { let o = off; off += s as usize; o }).collect();
                    let got: Vec<u8> = covering(&sizes, start, end)
                        .into_iter()
                        .flat_map(|(i, r)| data[starts[i] + r.start as usize..starts[i] + r.end as usize].to_vec())
                        .collect();
                    proptest::prop_assert_eq!(&got[..], &data[start as usize..end as usize]);
                }
                Unsatisfiable => proptest::prop_assert!(total == 0 || (!suffix && a >= total) || (suffix && a == 0)),
                Full => proptest::prop_assert!(!suffix && b < a || suffix && total == 0),
            }
        }
    }
}
