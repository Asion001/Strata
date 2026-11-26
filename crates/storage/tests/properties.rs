//! Property tests of the pure parts: the FastCDC chunker (deterministic,
//! and the browser's chunker cuts at the same places) and path
//! normalization (idempotent, never escapes the root).

use proptest::prelude::*;
use strata_storage::chunker::{self, MAX_SIZE, MIN_SIZE, SINGLE_BLOB_MAX};
use strata_storage::paths;

/// Bytes with some structure (runs and repeats) as well as noise: real
/// files are not uniformly random, and cut points depend on content.
fn data() -> impl Strategy<Value = Vec<u8>> {
    prop_oneof![
        // small, including the empty file
        prop::collection::vec(any::<u8>(), 0..4096),
        // around the chunked threshold and beyond (a seed and a length,
        // expanded, so large inputs stay cheap to generate and shrink)
        (
            any::<u64>(),
            prop_oneof![
                Just(SINGLE_BLOB_MAX - 1),
                Just(SINGLE_BLOB_MAX),
                Just(SINGLE_BLOB_MAX + 1),
                Just(MAX_SIZE - 1),
                Just(MAX_SIZE),
                Just(MAX_SIZE + 1),
                (MIN_SIZE..3 * MAX_SIZE),
            ]
        )
            .prop_map(|(seed, len)| noise(seed, len)),
        // long runs (no cut point found: the max size cuts)
        (any::<u8>(), MIN_SIZE..2 * MAX_SIZE).prop_map(|(b, len)| vec![b; len]),
    ]
}

fn noise(mut x: u64, len: usize) -> Vec<u8> {
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect()
}

proptest! {
    #![proptest_config(ProptestConfig { cases: std::env::var("PROPTEST_CASES").ok().and_then(|c| c.parse().ok()).unwrap_or(64).min(512), ..ProptestConfig::default() })]

    /// Same bytes, same cuts; the cuts cover the input exactly, every
    /// chunk within the size limits; the streaming chunker agrees.
    #[test]
    fn chunking_is_deterministic(d in data()) {
        let a = chunker::chunk_lengths(&d);
        prop_assert_eq!(&a, &chunker::chunk_lengths(&d));
        prop_assert_eq!(a.iter().sum::<usize>(), d.len());
        let n = a.len();
        for (i, &l) in a.iter().enumerate() {
            prop_assert!(l <= MAX_SIZE);
            if i + 1 < n {
                prop_assert!(l >= MIN_SIZE);
            }
        }
        // Fed in uneven pieces, the streaming chunker cuts the same.
        let mut s = chunker::StreamChunker::new();
        let mut got = Vec::new();
        for piece in d.chunks(777_777) {
            got.extend(s.push(piece).iter().map(Vec::len));
        }
        got.extend(s.finish().iter().map(Vec::len));
        prop_assert_eq!(got, a);
    }

    /// normalize(normalize(x)) == normalize(x), and a normalized path has no
    /// empty, "." or ".." components and no leading slash: it stays below
    /// the root it is joined to.
    #[test]
    fn normalize_is_idempotent_and_stays_inside(parts in prop::collection::vec(
        prop_oneof![
            Just("".to_string()), Just(".".to_string()), Just("..".to_string()),
            "[a-zA-Z0-9 ._\\-]{1,12}", "\\PC{1,8}", Just("a/b".to_string()),
        ], 0..8), lead in any::<bool>())
    {
        let input = format!("{}{}", if lead { "/" } else { "" }, parts.join("/"));
        if let Ok(n) = paths::normalize(&input) {
            prop_assert_eq!(paths::normalize(&n).unwrap(), n.clone());
            prop_assert!(!n.starts_with('/') && !n.ends_with('/'));
            for c in n.split('/').filter(|_| !n.is_empty()) {
                prop_assert!(!c.is_empty() && c != "." && c != "..", "{n:?}");
            }
            // Joined to a root, it never leaves it.
            let root = std::path::Path::new("/srv/root");
            let joined = root.join(&n);
            prop_assert!(joined.starts_with(root));
            prop_assert!(joined.components().all(|c| !matches!(c, std::path::Component::ParentDir)));
        } else {
            // Refused inputs hold a component that would mean something else.
            prop_assert!(input.split('/').any(|c| c == "." || c == ".." || c.len() > 255 || c.chars().any(char::is_control)) || input.len() > 4096);
        }
    }
}

/// The browser's chunker (web/src/lib/chunker.ts) cuts at exactly the same
/// places, for random inputs (runs Node; needs it in CI).
#[test]
fn browser_chunker_agrees() {
    let node = std::process::Command::new("node").arg("--version").output();
    if node.is_err() {
        if std::env::var_os("CI").is_some() {
            panic!("node is needed for this test in CI");
        }
        eprintln!("skipping: no node");
        return;
    }
    let cases = std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|c| c.parse::<usize>().ok())
        .unwrap_or(16)
        .clamp(8, 64);
    let mut runner = proptest::test_runner::TestRunner::new(ProptestConfig {
        cases: cases as u32,
        ..ProptestConfig::default()
    });
    let dir = tempfile::tempdir().unwrap();
    let script = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../web/scripts/chunk-lengths.ts"
    );
    runner
        .run(&data(), |d| {
            let f = dir.path().join("input.bin");
            std::fs::write(&f, &d).unwrap();
            let out = std::process::Command::new("node")
                .args(["--experimental-strip-types", "--no-warnings", script])
                .arg(&f)
                .output()
                .unwrap();
            prop_assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            let js: Vec<usize> = serde_json::from_slice(&out.stdout).unwrap();
            prop_assert_eq!(js, chunker::chunk_lengths(&d));
            Ok(())
        })
        .unwrap();
}
