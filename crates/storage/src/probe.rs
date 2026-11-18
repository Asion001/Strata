//! Health probe of a connection: can its root be read, and (when it may be
//! written) does a tiny write, read-back and delete work?
//!
//! The write test only ever touches files named `probe-<random>.tmp` inside
//! the reserved `.strata-probe/` folder at the connection's root (the server
//! never lets users create anything there). The file is deleted whatever
//! happened; files an interrupted probe left behind are swept by the next.

use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

use opendal::{ErrorKind, Operator};
use serde::{Deserialize, Serialize};

use crate::backend::PROBE_DIR;

const STEP_TIMEOUT: Duration = Duration::from_secs(20);
/// Leftovers older than this are swept (younger ones may be another
/// connection's probe in progress on the same storage).
const STALE: Duration = Duration::from_secs(600);

/// Why a probe failed, in words safe for anyone to see.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    Unreachable,
    TimedOut,
    AccessDenied,
    NotFound,
    Misconfigured,
    WriteFailed,
    Failed,
}

impl Reason {
    pub fn as_str(self) -> &'static str {
        match self {
            Reason::Unreachable => "unreachable",
            Reason::TimedOut => "timed out",
            Reason::AccessDenied => "access denied",
            Reason::NotFound => "not found",
            Reason::Misconfigured => "misconfigured",
            Reason::WriteFailed => "write test failed",
            Reason::Failed => "failed",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub reason: Reason,
    /// The backend's message, already sanitized (see `sanitize`).
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Readable, and writable if a write test ran.
    Ok,
    /// Readable, but the write test failed.
    ReadOnly(Failure),
    /// The root can't be read.
    Down(Failure),
}

#[derive(Clone, Debug)]
pub struct Probe {
    pub outcome: Outcome,
    /// How long reading the root took (the whole probe on failure).
    pub latency: Duration,
}

/// Probe a connection. `secrets` are credentials to scrub from messages.
/// `local_root` is a local connection's folder: it must exist (an unmounted
/// disk otherwise lists as an empty folder, and the write test would create
/// the folder again on the wrong disk).
pub async fn probe(
    op: &Operator,
    local_root: Option<&Path>,
    write: bool,
    secrets: &[&str],
) -> Probe {
    let t0 = Instant::now();
    let read = step(async {
        if let Some(root) = local_root {
            match tokio::fs::metadata(root).await {
                Ok(m) if m.is_dir() => {}
                Ok(_) => {
                    return Err(StepError::Missing(
                        "the connection's folder is not a folder",
                    ));
                }
                Err(_) => return Err(StepError::Missing("the connection's folder is missing")),
            }
        }
        // One entry is enough to prove the root lists.
        let mut l = op.lister_with("/").limit(1).await?;
        let _ = futures::TryStreamExt::try_next(&mut l).await?;
        Ok(())
    })
    .await;
    let latency = t0.elapsed();
    if let Err(e) = read {
        return Probe {
            outcome: Outcome::Down(failure(e, secrets)),
            latency,
        };
    }
    if !write {
        return Probe {
            outcome: Outcome::Ok,
            latency,
        };
    }
    let outcome = match write_check(op, secrets).await {
        Ok(()) => Outcome::Ok,
        Err(f) => Outcome::ReadOnly(f),
    };
    Probe { outcome, latency }
}

/// The write half of a probe on its own: write, read back and delete one
/// small file under the probe folder.
pub async fn write_check(op: &Operator, secrets: &[&str]) -> Result<(), Failure> {
    write_test(op).await.map_err(|e| {
        let mut f = failure(e, secrets);
        if f.reason == Reason::Failed {
            f.reason = Reason::WriteFailed;
        }
        f
    })
}

enum StepError {
    Backend(opendal::Error),
    Missing(&'static str),
    TimedOut,
    Mismatch,
}

impl From<opendal::Error> for StepError {
    fn from(e: opendal::Error) -> Self {
        StepError::Backend(e)
    }
}

async fn step<T>(
    f: impl std::future::Future<Output = Result<T, StepError>>,
) -> Result<T, StepError> {
    tokio::time::timeout(STEP_TIMEOUT, f)
        .await
        .unwrap_or(Err(StepError::TimedOut))
}

fn probe_name(name: &str) -> bool {
    name.strip_prefix("probe-")
        .and_then(|r| r.strip_suffix(".tmp"))
        .is_some_and(|h| h.len() == 16 && h.bytes().all(|b| b.is_ascii_hexdigit()))
}

async fn write_test(op: &Operator) -> Result<(), StepError> {
    let dir = format!("{PROBE_DIR}/");
    sweep(op, &dir).await;
    let tag: u64 = rand::random();
    let path = format!("{dir}probe-{tag:016x}.tmp");
    let data: [u8; 16] = rand::random();
    let res = step(async {
        op.write(&path, data.to_vec()).await?;
        let back = op.read(&path).await?.to_vec();
        if back != data {
            return Err(StepError::Mismatch);
        }
        Ok(())
    })
    .await;
    // Always delete, whatever happened above (a timed-out write may still
    // have landed).
    let del = step(async { Ok(op.delete(&path).await?) }).await;
    // The folder goes too when nothing else is in it (on a filesystem; S3
    // has no folders). Checked first: deleting a non-empty folder fails and
    // would only be retried.
    let empty = step(async {
        let l = op.list(&dir).await?;
        Ok(l.iter().all(|e| e.path() == dir))
    })
    .await
    .unwrap_or(false);
    if empty {
        let _ = step(async { Ok(op.delete(&dir).await?) }).await;
    }
    res.and(del)
}

/// Remove probe files an interrupted probe left behind.
async fn sweep(op: &Operator, dir: &str) {
    let Ok(Ok(entries)) = tokio::time::timeout(STEP_TIMEOUT, op.list(dir)).await else {
        return;
    };
    let now = SystemTime::now();
    for e in entries {
        if !probe_name(e.name()) {
            continue;
        }
        let modified = match e.metadata().last_modified() {
            Some(t) => Some(SystemTime::from(t)),
            None => match tokio::time::timeout(STEP_TIMEOUT, op.stat(e.path())).await {
                Ok(Ok(m)) => m.last_modified().map(SystemTime::from),
                _ => None,
            },
        };
        let old = modified.is_some_and(|t| now.duration_since(t).unwrap_or_default() > STALE);
        if old {
            let _ = tokio::time::timeout(STEP_TIMEOUT, op.delete(e.path())).await;
        }
    }
}

fn failure(e: StepError, secrets: &[&str]) -> Failure {
    match e {
        StepError::TimedOut => Failure {
            reason: Reason::TimedOut,
            detail: format!("timed out after {}s", STEP_TIMEOUT.as_secs()),
        },
        StepError::Missing(msg) => Failure {
            reason: Reason::NotFound,
            detail: msg.into(),
        },
        StepError::Mismatch => Failure {
            reason: Reason::WriteFailed,
            detail: "read back different bytes than were written".into(),
        },
        StepError::Backend(e) => {
            let text = format!("{e:#}");
            Failure {
                reason: classify(e.kind(), &text),
                detail: sanitize(&text, secrets),
            }
        }
    }
}

fn classify(kind: ErrorKind, text: &str) -> Reason {
    match kind {
        ErrorKind::PermissionDenied => Reason::AccessDenied,
        ErrorKind::NotFound => Reason::NotFound,
        ErrorKind::ConfigInvalid => Reason::Misconfigured,
        _ => {
            let t = text.to_ascii_lowercase();
            if t.contains("timed out") || t.contains("timeout") {
                Reason::TimedOut
            } else if [
                "connect",
                "dns",
                "refused",
                "unreachable",
                "network",
                "resolve",
                "broken pipe",
                "reset",
                "sending request",
                "send http request",
            ]
            .iter()
            .any(|w| t.contains(w))
            {
                Reason::Unreachable
            } else if t.contains("accessdenied") || t.contains("forbidden") || t.contains("403") {
                Reason::AccessDenied
            } else {
                Reason::Failed
            }
        }
    }
}

/// Make a backend error safe to store and show to admins: credentials,
/// `user:password@` parts of URLs and query strings (which can carry
/// signatures) are removed, and the length is capped. Hostnames stay: only
/// admins ever see this text; everyone else sees the `Reason`.
pub fn sanitize(text: &str, secrets: &[&str]) -> String {
    let mut s = text.to_string();
    for sec in secrets.iter().filter(|s| s.len() >= 4) {
        s = s.replace(sec, "[redacted]");
    }
    let mut out = String::with_capacity(s.len());
    for (i, word) in s.split(' ').enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(&scrub_url(word));
    }
    // Credential-looking key=value pairs anywhere.
    let out = scrub_pairs(&out);
    let mut out: String = out
        .chars()
        .filter(|c| !c.is_control() || *c == ' ')
        .collect();
    if out.chars().count() > 300 {
        out = out.chars().take(300).collect::<String>() + "…";
    }
    out
}

fn scrub_url(word: &str) -> String {
    let Some(scheme_end) = word.find("://") else {
        return word.to_string();
    };
    let (head, rest) = word.split_at(scheme_end + 3);
    let (authority, tail) = match rest.find(['/', '?', '#']) {
        Some(i) => rest.split_at(i),
        None => (rest, ""),
    };
    let authority = match authority.rfind('@') {
        Some(i) => &authority[i + 1..],
        None => authority,
    };
    let tail = match tail.find('?') {
        Some(i) => format!("{}?[redacted]", &tail[..i]),
        None => tail.to_string(),
    };
    format!("{head}{authority}{tail}")
}

fn scrub_pairs(s: &str) -> String {
    const KEYS: [&str; 8] = [
        "x-amz-credential",
        "x-amz-signature",
        "signature",
        "credential",
        "password",
        "secret",
        "token",
        "authorization",
    ];
    let lower = s.to_ascii_lowercase();
    let mut out = String::new();
    let mut i = 0;
    while i < s.len() {
        let hit = KEYS.iter().find_map(|k| {
            lower[i..].starts_with(k).then(|| {
                let after = i + k.len();
                let sep = s[after..].chars().next()?;
                (sep == '=' || sep == ':').then_some(after + 1)
            })?
        });
        if let Some(v) = hit {
            out.push_str(&s[i..v]);
            let rest = &s[v..];
            let rest_trim = rest.trim_start();
            let skip_ws = rest.len() - rest_trim.len();
            let end = rest_trim
                .find(|c: char| c.is_whitespace() || c == ',' || c == '&' || c == '"' || c == ';')
                .unwrap_or(rest_trim.len());
            out.push_str(&rest[..skip_ws]);
            out.push_str("[redacted]");
            i = v + skip_ws + end;
        } else {
            let c = s[i..].chars().next().unwrap();
            out.push(c);
            i += c.len_utf8();
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizing() {
        let s = sanitize(
            "PermissionDenied at https://AKIAXYZ:hunter2@minio.local:9000/b/x?X-Amz-Signature=abc&X-Amz-Credential=AKIAXYZ%2F2026 with key AKIAXYZ and secret=topsecret password: pw123",
            &["AKIAXYZ", "topsecret"],
        );
        assert!(!s.contains("AKIAXYZ"), "{s}");
        assert!(!s.contains("hunter2"), "{s}");
        assert!(!s.contains("topsecret"), "{s}");
        assert!(!s.contains("pw123"), "{s}");
        assert!(!s.contains("abc"), "{s}");
        assert!(s.contains("minio.local:9000/b/x?[redacted]"), "{s}");
        assert!(sanitize(&"x".repeat(1000), &[]).chars().count() <= 301);
        // short "secrets" (e.g. an empty password) are not blanked everywhere
        assert_eq!(sanitize("a b c", &["", "a"]), "a b c");
    }

    #[test]
    fn classification() {
        assert_eq!(
            classify(ErrorKind::PermissionDenied, ""),
            Reason::AccessDenied
        );
        assert_eq!(
            classify(
                ErrorKind::Unexpected,
                "error sending request: Connection refused"
            ),
            Reason::Unreachable
        );
        assert_eq!(
            classify(ErrorKind::Unexpected, "operation timed out"),
            Reason::TimedOut
        );
        assert_eq!(classify(ErrorKind::Unexpected, "weird"), Reason::Failed);
        assert_eq!(
            classify(
                ErrorKind::Unexpected,
                "Unexpected (persistent) at list => send http request, source: error sending request"
            ),
            Reason::Unreachable
        );
    }

    #[test]
    fn probe_names() {
        assert!(probe_name("probe-0123456789abcdef.tmp"));
        assert!(!probe_name("probe-0123.tmp"));
        assert!(!probe_name("notes.txt"));
        assert!(!probe_name("probe-0123456789abcdeg.tmp"));
    }

    #[tokio::test]
    async fn local_probe_leaves_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let op = crate::ConnectionConfig::Local { path: "".into() }
            .operator(dir.path())
            .unwrap();
        // a stale leftover and a user file nearby
        std::fs::create_dir_all(dir.path().join(PROBE_DIR)).unwrap();
        let stale = dir
            .path()
            .join(PROBE_DIR)
            .join("probe-00000000000000aa.tmp");
        std::fs::write(&stale, b"x").unwrap();
        let old = std::time::SystemTime::now() - Duration::from_secs(3600);
        std::fs::File::options()
            .write(true)
            .open(&stale)
            .unwrap()
            .set_modified(old)
            .unwrap();
        std::fs::write(dir.path().join("keep.txt"), b"mine").unwrap();
        let p = probe(&op, Some(dir.path()), true, &[]).await;
        assert_eq!(p.outcome, Outcome::Ok);
        assert!(
            !dir.path().join(PROBE_DIR).exists(),
            "probe folder left behind"
        );
        assert_eq!(std::fs::read(dir.path().join("keep.txt")).unwrap(), b"mine");
        // a root that goes missing after its operator was built
        let cfg = crate::ConnectionConfig::Local {
            path: "nope/deeper".into(),
        };
        assert!(
            cfg.operator(dir.path()).is_err(),
            "no operator for a missing folder"
        );
        assert!(!dir.path().join("nope").exists(), "building recreated it");
        cfg.create_local_root(dir.path()).unwrap();
        let gone = cfg.operator(dir.path()).unwrap();
        std::fs::remove_dir_all(dir.path().join("nope")).unwrap();
        let p = probe(&gone, Some(&dir.path().join("nope/deeper")), true, &[]).await;
        assert!(matches!(p.outcome, Outcome::Down(_)), "{:?}", p.outcome);
        assert!(
            !dir.path().join("nope").exists(),
            "the probe recreated a missing root"
        );
    }
}
