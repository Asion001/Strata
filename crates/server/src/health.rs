//! Connection health: a probe per connection every `probe_secs` (jittered).
//!
//! Each probe reads the root and, when the connection may be written, writes,
//! reads back and deletes one small file under `.strata-probe/` (see
//! `strata_storage::probe`). Capabilities are detected again too.
//!
//! States: `ok`; `degraded` (the write test failed, or a read failed fewer
//! than three times in a row); `offline` (three failed reads in a row). One
//! successful read ends `offline`. While offline, the connection's storage is
//! not touched: reads and writes through its mounts get 503 (`AppState::
//! operator`), its mount points still list (flagged offline) and hashed
//! mounts still list from the database.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rand::Rng;
use strata_storage::probe::{self, Outcome, Reason};
use strata_storage::{Capabilities, ConnectionConfig};

use crate::state::{AppState, ChangeEvent};

/// Failed reads in a row that make a connection offline.
pub const OFFLINE_AFTER: i32 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Unknown,
    Ok,
    Degraded,
    Offline,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Unknown => "unknown",
            State::Ok => "ok",
            State::Degraded => "degraded",
            State::Offline => "offline",
        }
    }
    pub fn parse(s: &str) -> State {
        match s {
            "ok" => State::Ok,
            "degraded" => State::Degraded,
            "offline" => State::Offline,
            _ => State::Unknown,
        }
    }
}

/// What a probe found, for the state machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Found {
    Ok,
    /// Readable, but the write test failed.
    ReadOnly,
    /// The root couldn't be read.
    Down,
}

/// The state after a probe, and the failed reads in a row so far.
pub fn next(failures: i32, found: Found) -> (State, i32) {
    match found {
        Found::Ok => (State::Ok, 0),
        Found::ReadOnly => (State::Degraded, 0),
        Found::Down => {
            let f = failures.saturating_add(1);
            (
                if f >= OFFLINE_AFTER {
                    State::Offline
                } else {
                    State::Degraded
                },
                f,
            )
        }
    }
}

/// Mark the connections the database says are offline (at start-up).
pub async fn load(st: &AppState) -> sqlx::Result<()> {
    let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM connections WHERE health = 'offline'")
        .fetch_all(&st.db)
        .await?;
    for id in ids {
        st.offline.insert(id, ());
    }
    Ok(())
}

/// A connection's credentials, to scrub from error messages.
pub fn secrets(cfg: &ConnectionConfig) -> Vec<String> {
    match cfg {
        ConnectionConfig::Local { .. } => vec![],
        ConnectionConfig::S3 {
            access_key_id,
            secret_access_key,
            ..
        } => vec![access_key_id.clone(), secret_access_key.clone()],
        ConnectionConfig::Ftp { user, password, .. } => {
            user.iter().chain(password.iter()).cloned().collect()
        }
    }
}

/// Probe one connection now and record the result; returns the failed
/// reads in a row afterwards (None if skipped or it failed to record). One
/// probe per connection at a time: `wait` false skips if one is running.
pub async fn probe_one(st: &AppState, id: i64, wait: bool, counted: bool) -> Option<i32> {
    let lock = st
        .probe_locks
        .entry(id)
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone();
    let _guard = if wait {
        lock.lock().await
    } else {
        match lock.try_lock() {
            Ok(g) => g,
            Err(_) => return None,
        }
    };
    match probe_inner(st, id, counted).await {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!("probe of connection {id}: {e:#}");
            None
        }
    }
}

/// Probe now for a request (add, edit, "Check"): wait up to 25 seconds,
/// then answer anyway (the probe finishes in the background and its
/// result shows on the next load or arrives over the websocket).
pub async fn probe_soon(st: &AppState, id: i64) {
    let s = st.clone();
    let task = tokio::spawn(async move { probe_one(&s, id, true, false).await });
    let _ = tokio::time::timeout(Duration::from_secs(25), task).await;
}

/// Longest a probe may take before it counts as failed (each step has its
/// own shorter limit; this bounds them all, so the probe lock is released).
const PROBE_LIMIT: Duration = Duration::from_secs(120);

async fn probe_inner(st: &AppState, id: i64, counted: bool) -> anyhow::Result<Option<i32>> {
    type Row = (Vec<u8>, String, i32, sqlx::types::Json<serde_json::Value>);
    let row: Option<Row> = sqlx::query_as(
        "SELECT config_enc, health, health_failures, capabilities FROM connections WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&st.db)
    .await?;
    let Some((enc, prev, failures, prev_caps)) = row else {
        return Ok(None); // deleted meanwhile
    };
    let prev = State::parse(&prev);
    // An empty column (new or re-pointed connection) has no detection yet.
    let prev_caps: Option<Capabilities> = prev_caps
        .0
        .as_object()
        .filter(|o| !o.is_empty())
        .and_then(|_| serde_json::from_value(prev_caps.0.clone()).ok());

    let mut caps: Option<Capabilities> = None;
    let mut marker: Option<PathBuf> = None;
    let probed = tokio::time::timeout(PROBE_LIMIT, async {
        match st.decrypt_config(&enc) {
            Err(_) => (
                Found::Down,
                None,
                Some(probe::Failure {
                    reason: Reason::Misconfigured,
                    detail: "the stored settings can't be read (server key?)".into(),
                }),
            ),
            Ok(cfg) => match async {
                // A local folder must exist (and be the right one) before its
                // operator is even built.
                if let Some(root) = st.local_root(id, &cfg).await.map_err(Ok)? {
                    st.check_root(id, root.clone(), Duration::from_secs(20))
                        .await
                        .map_err(|detail| {
                            Err(probe::Failure {
                                reason: Reason::NotFound,
                                detail,
                            })
                        })?;
                    if root.marker.is_none() {
                        marker = Some(crate::state::LocalRoot::marker_path(&root.dir, id));
                    }
                }
                st.operator_unchecked(id).await.map_err(Ok)
            }
            .await
            {
                Err(Err(f)) => (Found::Down, None, Some(f)),
                Err(Ok(e)) => (
                    Found::Down,
                    None,
                    Some(probe::Failure {
                        reason: Reason::Misconfigured,
                        detail: probe::sanitize(&e.to_string(), &[]),
                    }),
                ),
                Ok(op) => {
                    let secrets = secrets(&cfg);
                    let secrets: Vec<&str> = secrets.iter().map(String::as_str).collect();
                    // (A local folder was checked above.)
                    let p = probe::probe(&op, None, false, &secrets).await;
                    match p.outcome {
                        Outcome::Down(f) | Outcome::ReadOnly(f) => {
                            (Found::Down, Some(p.latency), Some(f))
                        }
                        Outcome::Ok => {
                            let c = Capabilities::detect(
                                &cfg,
                                &op,
                                &st.cfg.local_base,
                                prev_caps.as_ref(),
                            )
                            .await;
                            // A write test only where a delete leaves nothing
                            // behind (not on a versioned bucket, nor where that
                            // can't be confirmed).
                            let write = c.clean_deletes;
                            caps = Some(c);
                            if !write {
                                (Found::Ok, Some(p.latency), None)
                            } else {
                                match probe::write_check(&op, &secrets).await {
                                    Ok(()) => (Found::Ok, Some(p.latency), None),
                                    Err(f) => (Found::ReadOnly, Some(p.latency), Some(f)),
                                }
                            }
                        }
                    }
                }
            },
        }
    })
    .await;
    let (found, latency, failure) = probed.unwrap_or_else(|_| {
        (
            Found::Down,
            None,
            Some(probe::Failure {
                reason: Reason::TimedOut,
                detail: format!("the probe took over {}s", PROBE_LIMIT.as_secs()),
            }),
        )
    });

    // A manual check ("Check", adding or editing) never counts towards
    // offline: clicking it during a blip mustn't take the storage away from
    // everyone. A success still counts (it ends offline).
    let (state, failures) = if !counted && found == Found::Down {
        (
            if prev == State::Offline {
                State::Offline
            } else {
                State::Degraded
            },
            failures,
        )
    } else {
        next(failures, found)
    };
    // None when no request reached the backend (shown as no latency).
    let latency_ms = latency.map(|l| i32::try_from(l.as_millis()).unwrap_or(i32::MAX));
    let written = sqlx::query(
        "UPDATE connections SET health = $2, health_failures = $3, health_latency_ms = $4,
                health_reason = $5, health_error = $6, health_checked_at = now(),
                capabilities = COALESCE($7, capabilities)
         WHERE id = $1 AND config_enc = $8",
    )
    .bind(id)
    .bind(state.as_str())
    .bind(failures)
    .bind(latency_ms)
    .bind(failure.as_ref().map(|f| f.reason.as_str()))
    .bind(failure.as_ref().map(|f| f.detail.as_str()))
    .bind(caps.map(sqlx::types::Json))
    .bind(&enc)
    .execute(&st.db)
    .await?;
    if written.rows_affected() == 0 {
        // The connection was edited (or deleted) while we probed: this
        // result is about the old config, so it is dropped.
        return Ok(None);
    }

    // A folder from before markers existed (new and re-pointed folders get
    // theirs when they are made) is adopted once it is seen healthy, but
    // only if it holds something: an empty folder may be the bare mount
    // point of a disk that isn't mounted right now, and marking that would
    // make it "the" folder.
    if found == Found::Ok
        && let Some(dir) = marker
            .as_ref()
            .and_then(|m| m.parent())
            .and_then(|p| p.parent())
    {
        let d = dir.to_path_buf();
        let has_content = tokio::task::spawn_blocking(move || {
            std::fs::read_dir(d).is_ok_and(|mut rd| {
                rd.any(|e| {
                    e.is_ok_and(|e| {
                        let n = e.file_name();
                        n != strata_storage::backend::PROBE_DIR
                            && n != strata_storage::backend::ATOMIC_TMP_DIR
                    })
                })
            })
        })
        .await
        .unwrap_or(false);
        if has_content {
            // Only for the config this probe saw (see above).
            if let Err(e) = st.mark_root(id, dir.to_path_buf(), Some(&enc)).await {
                tracing::warn!("connection {id}: can't write its folder marker: {e}");
            }
        }
    }

    if state == State::Offline {
        st.offline.insert(id, ());
    } else {
        st.offline.remove(&id);
    }
    if state == State::Ok && prev != State::Ok {
        // An operator built while the storage was away (a local folder
        // missing its temp folder) is rebuilt.
        st.forget_operator(id);
    }
    if state != prev {
        match &failure {
            Some(f) => tracing::warn!(
                "connection {id}: {} -> {} ({}: {})",
                prev.as_str(),
                state.as_str(),
                f.reason.as_str(),
                f.detail
            ),
            None => tracing::info!("connection {id}: {} -> {}", prev.as_str(), state.as_str()),
        }
        // Who hears about it: users with a mount on it, its owner, admins.
        let audience: Vec<i64> = sqlx::query_scalar(
            "SELECT m.owner_id FROM mounts m JOIN volumes v ON v.id = m.volume_id WHERE v.connection_id = $1
             UNION SELECT owner_id FROM connections WHERE id = $1
             UNION SELECT id FROM users WHERE is_admin",
        )
        .bind(id)
        .fetch_all(&st.db)
        .await
        .unwrap_or_default();
        let _ = st.events.send(ChangeEvent {
            volume_id: None,
            user_id: None,
            connection_id: Some(id),
            audience: Some(Arc::new(audience)),
        });
    }
    Ok(Some(failures))
}

/// Time from a probe to the connection's next one: the interval minus up to
/// 10% jitter (never more), and half of that while reads are failing but it
/// isn't offline yet. Computed from the probe's own result, so a backend
/// that dies is offline within two intervals plus the probes' run time.
pub fn delay(every: Duration, failures: i32, jitter: f64) -> Duration {
    let d = every.mul_f64(1.0 - 0.1 * jitter.clamp(0.0, 1.0));
    if (1..OFFLINE_AFTER).contains(&failures) {
        d / 2
    } else {
        d
    }
}

/// Probe every connection on its own jittered schedule (see `delay`), at
/// most one probe per connection at a time.
pub async fn run(st: AppState) {
    let every = Duration::from_secs(st.cfg.probe_secs);
    let due: Arc<std::sync::Mutex<HashMap<i64, Instant>>> = Default::default();
    let mut tick = tokio::time::interval(Duration::from_secs(5));
    loop {
        tick.tick().await;
        let ids: Vec<i64> = match sqlx::query_scalar("SELECT id FROM connections")
            .fetch_all(&st.db)
            .await
        {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("health probes: {e}");
                continue;
            }
        };
        let now = Instant::now();
        let mut start = Vec::new();
        {
            let mut due = due.lock().unwrap();
            let live: std::collections::HashSet<i64> = ids.iter().copied().collect();
            due.retain(|id, _| live.contains(id));
            for id in ids {
                // New connections (and all at start-up) spread over a few seconds.
                let at = due.entry(id).or_insert_with(|| {
                    now + Duration::from_millis(rand::thread_rng().gen_range(0..5000))
                });
                if *at <= now {
                    // Rescheduled by the probe itself, from its result.
                    *at = now + Duration::from_secs(3600);
                    start.push(id);
                }
            }
        }
        for id in start {
            let (s, due) = (st.clone(), due.clone());
            tokio::spawn(async move {
                let next = match probe_one(&s, id, false, true).await {
                    Some(failures) => {
                        delay(every, failures, rand::thread_rng().gen_range(0.0..1.0))
                    }
                    // Busy (a manual probe) or not recorded: soon again.
                    None => Duration::from_secs(5),
                };
                if let Some(at) = due.lock().unwrap().get_mut(&id) {
                    *at = Instant::now() + next;
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_failed_reads_make_offline_one_success_ends_it() {
        let (s, f) = next(0, Found::Down);
        assert_eq!((s, f), (State::Degraded, 1));
        let (s, f) = next(f, Found::Down);
        assert_eq!((s, f), (State::Degraded, 2));
        let (s, f) = next(f, Found::Down);
        assert_eq!((s, f), (State::Offline, 3));
        let (s, f) = next(f, Found::Down);
        assert_eq!((s, f), (State::Offline, 4));
        assert_eq!(next(f, Found::Ok), (State::Ok, 0));
        // a readable but read-only backend is degraded, never offline
        let (s, f) = next(2, Found::ReadOnly);
        assert_eq!((s, f), (State::Degraded, 0));
        assert_eq!(next(f, Found::Down), (State::Degraded, 1));
        assert_eq!(next(i32::MAX, Found::Down).0, State::Offline);
    }

    #[test]
    fn schedule() {
        let m = Duration::from_secs(60);
        assert_eq!(delay(m, 0, 0.0), m);
        assert_eq!(delay(m, 0, 1.0), Duration::from_secs(54));
        // failing, not offline yet: twice as often
        assert_eq!(delay(m, 1, 0.0), Duration::from_secs(30));
        assert_eq!(delay(m, 2, 0.0), Duration::from_secs(30));
        // offline: back to the normal interval (back within one interval)
        assert_eq!(delay(m, 3, 0.0), m);
        // worst case from a backend dying to offline: one full interval to
        // the first failed probe, then two halves
        let worst = delay(m, 0, 0.0) + delay(m, 1, 0.0) + delay(m, 2, 0.0);
        assert!(worst <= Duration::from_secs(120));
    }

    #[test]
    fn state_names_round_trip() {
        for s in [State::Unknown, State::Ok, State::Degraded, State::Offline] {
            assert_eq!(State::parse(s.as_str()), s);
        }
        assert_eq!(State::parse("error"), State::Unknown);
    }
}
