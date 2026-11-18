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
use std::sync::Arc;
use std::time::{Duration, Instant};

use rand::Rng;
use strata_storage::probe::{self, Outcome, Reason};
use strata_storage::{Capabilities, ConnectionConfig, SnapshotKind};

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

/// Probe one connection now and record the result. One probe per
/// connection at a time: `wait` false skips if one is running.
pub async fn probe_one(st: &AppState, id: i64, wait: bool) {
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
            Err(_) => return,
        }
    };
    if let Err(e) = probe_inner(st, id).await {
        tracing::warn!("probe of connection {id}: {e:#}");
    }
}

async fn probe_inner(st: &AppState, id: i64) -> anyhow::Result<()> {
    let row: Option<(Vec<u8>, String, i32)> =
        sqlx::query_as("SELECT config_enc, health, health_failures FROM connections WHERE id = $1")
            .bind(id)
            .fetch_optional(&st.db)
            .await?;
    let Some((enc, prev, failures)) = row else {
        return Ok(()); // deleted meanwhile
    };
    let prev = State::parse(&prev);

    let mut caps: Option<Capabilities> = None;
    let (found, latency, failure) = match st.decrypt_config(&enc) {
        Err(_) => (
            Found::Down,
            Duration::ZERO,
            Some(probe::Failure {
                reason: Reason::Misconfigured,
                detail: "the stored settings can't be read (server key?)".into(),
            }),
        ),
        Ok(cfg) => match st.operator_unchecked(id).await {
            Err(e) => (
                Found::Down,
                Duration::ZERO,
                Some(probe::Failure {
                    reason: Reason::Misconfigured,
                    detail: probe::sanitize(&e.to_string(), &[]),
                }),
            ),
            Ok(op) => {
                let secrets = secrets(&cfg);
                let secrets: Vec<&str> = secrets.iter().map(String::as_str).collect();
                let root = cfg.local_dir(&st.cfg.local_base).ok().flatten();
                let p = probe::probe(&op, root.as_deref(), false, &secrets).await;
                match p.outcome {
                    Outcome::Down(f) | Outcome::ReadOnly(f) => (Found::Down, p.latency, Some(f)),
                    Outcome::Ok => {
                        let c = Capabilities::detect(&cfg, &op, &st.cfg.local_base).await;
                        // No write test on a versioned bucket: deleting the
                        // probe file there would leave versions behind.
                        let write = c.snapshots.kind != SnapshotKind::S3Versioning;
                        caps = Some(c);
                        if !write {
                            (Found::Ok, p.latency, None)
                        } else {
                            match probe::write_check(&op, &secrets).await {
                                Ok(()) => (Found::Ok, p.latency, None),
                                Err(f) => (Found::ReadOnly, p.latency, Some(f)),
                            }
                        }
                    }
                }
            }
        },
    };

    let (state, failures) = next(failures, found);
    let latency_ms = i32::try_from(latency.as_millis()).unwrap_or(i32::MAX);
    sqlx::query(
        "UPDATE connections SET health = $2, health_failures = $3, health_latency_ms = $4,
                health_reason = $5, health_error = $6, health_checked_at = now(),
                capabilities = COALESCE($7, capabilities)
         WHERE id = $1",
    )
    .bind(id)
    .bind(state.as_str())
    .bind(failures)
    .bind(latency_ms)
    .bind(failure.as_ref().map(|f| f.reason.as_str()))
    .bind(failure.as_ref().map(|f| f.detail.as_str()))
    .bind(caps.map(sqlx::types::Json))
    .execute(&st.db)
    .await?;

    if state == State::Offline {
        st.offline.insert(id, ());
    } else {
        st.offline.remove(&id);
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
        let _ = st.events.send(ChangeEvent {
            volume_id: None,
            user_id: None,
            connection_id: Some(id),
        });
    }
    Ok(())
}

/// Time to a connection's next probe: the interval minus up to 10% jitter
/// (never more, so a dead backend is offline within three intervals), and
/// half of that while reads are failing but it isn't offline yet.
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
    let mut due: HashMap<i64, Instant> = HashMap::new();
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    loop {
        tick.tick().await;
        let rows: Vec<(i64, i32)> =
            match sqlx::query_as("SELECT id, health_failures FROM connections")
                .fetch_all(&st.db)
                .await
            {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!("health probes: {e}");
                    continue;
                }
            };
        due.retain(|id, _| rows.iter().any(|(r, _)| r == id));
        let now = Instant::now();
        for (id, failures) in rows {
            // New connections (and all at start-up) spread over a few seconds.
            let at = due.entry(id).or_insert_with(|| {
                now + Duration::from_millis(rand::thread_rng().gen_range(0..5000))
            });
            if *at > now {
                continue;
            }
            *at = now + delay(every, failures, rand::thread_rng().gen_range(0.0..1.0));
            let s = st.clone();
            tokio::spawn(async move { probe_one(&s, id, false).await });
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
