//! Change journal and its websocket push.
//!
//! Changes are recorded once per storage location (volume). Each volume, and
//! each user's own tree (mount points), has a gap-free sequence allocated
//! under a row lock in the transaction that inserts the rows, so sequence
//! order is commit order and a reader can never skip a change that commits
//! late. A user's feed merges the journals of every volume they mount with
//! their own rows; its cursor is an opaque token mapping scope -> seq.

use std::collections::{BTreeMap, HashMap};

use axum::Json;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use strata_storage::paths;

use crate::auth::AuthUser;
use crate::error::{ApiResult, bad};
use crate::mounts;
use crate::state::{AppState, ChangeEvent};

pub const NOTIFY_CHANNEL: &str = "strata_journal";
const RETENTION_DAYS: i32 = 30;
const RETENTION_ROWS: i64 = 1_000_000;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Change {
    pub op: String,
    pub path: String,
    pub new_path: Option<String>,
    pub is_dir: bool,
}

impl Change {
    pub fn new(op: &str, path: &str, new_path: Option<&str>, is_dir: bool) -> Self {
        Self {
            op: op.into(),
            path: path.into(),
            new_path: new_path.map(str::to_string),
            is_dir,
        }
    }
    pub fn create(path: &str, is_dir: bool) -> Self {
        Self::new("create", path, None, is_dir)
    }
    pub fn modify(path: &str) -> Self {
        Self::new("modify", path, None, false)
    }
    pub fn delete(path: &str, is_dir: bool) -> Self {
        Self::new("delete", path, None, is_dir)
    }
    pub fn moved(from: &str, to: &str, is_dir: bool) -> Self {
        Self::new("move", from, Some(to), is_dir)
    }
}

enum Scope {
    Volume(i64),
    User(i64),
}

async fn insert(st: &AppState, scope: Scope, changes: &[Change]) -> ApiResult<()> {
    if changes.is_empty() {
        return Ok(());
    }
    let n = changes.len() as i64;
    let mut tx = st.db.begin().await?;
    // Row lock on the counter: serialises writers of one scope until commit.
    let (vid, uid, last): (Option<i64>, Option<i64>, i64) = match scope {
        Scope::Volume(v) => (
            Some(v),
            None,
            sqlx::query_scalar(
                "UPDATE volumes SET journal_seq = journal_seq + $2 WHERE id = $1 RETURNING journal_seq",
            )
            .bind(v)
            .bind(n)
            .fetch_one(&mut *tx)
            .await?,
        ),
        Scope::User(u) => (
            None,
            Some(u),
            sqlx::query_scalar(
                "UPDATE users SET journal_seq = journal_seq + $2 WHERE id = $1 RETURNING journal_seq",
            )
            .bind(u)
            .bind(n)
            .fetch_one(&mut *tx)
            .await?,
        ),
    };
    let seqs: Vec<i64> = (last - n + 1..=last).collect();
    let ops: Vec<&str> = changes.iter().map(|c| c.op.as_str()).collect();
    let paths_: Vec<&str> = changes.iter().map(|c| c.path.as_str()).collect();
    let news: Vec<Option<&str>> = changes.iter().map(|c| c.new_path.as_deref()).collect();
    let dirs: Vec<bool> = changes.iter().map(|c| c.is_dir).collect();
    sqlx::query(
        "INSERT INTO journal (volume_id, user_id, seq, op, path, new_path, is_dir)
         SELECT $1, $2, * FROM unnest($3::int8[], $4::text[], $5::text[], $6::text[], $7::bool[])",
    )
    .bind(vid)
    .bind(uid)
    .bind(seqs)
    .bind(ops)
    .bind(paths_)
    .bind(news)
    .bind(dirs)
    .execute(&mut *tx)
    .await?;
    // Wakes websocket listeners on every server instance (ids only).
    let payload = json!({ "v": vid, "u": uid }).to_string();
    sqlx::query("SELECT pg_notify($1, $2)")
        .bind(NOTIFY_CHANNEL)
        .bind(payload)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    // Wake this instance's sockets directly too, in case the listener is
    // reconnecting (duplicates are harmless: clients debounce).
    let _ = st.events.send(ChangeEvent {
        volume_id: vid,
        user_id: uid,
        connection_id: None,
        audience: None,
    });
    Ok(())
}

/// Record changes to one user's tree itself (mount points added, moved or
/// removed). Paths are in that user's path space.
pub async fn record_user(st: &AppState, user_id: i64, changes: Vec<Change>) -> ApiResult<()> {
    insert(st, Scope::User(user_id), &changes).await
}

/// Map a change made in a volume based at `base` into an overlapping raw
/// volume based at `other` on the same connection. `None` if it is not
/// visible there.
pub fn map_into_overlap(base: &str, other: &str, c: &Change) -> Option<Change> {
    let abs = |rel: &str| paths::join(base, rel);
    let map = |rel: &str| paths::strip_prefix(&abs(rel), other).map(str::to_string);
    // The other volume's whole folder sits inside what was deleted or moved away.
    let swallows = |rel: &str| {
        let a = abs(rel);
        a != other && paths::strip_prefix(other, &a).is_some()
    };
    let from = map(&c.path);
    let to = c.new_path.as_deref().and_then(map);
    match (c.op.as_str(), from, to) {
        ("move", Some(f), Some(t)) => Some(Change::moved(&f, &t, c.is_dir)),
        // Moved out of the other volume's folder: gone from its view.
        ("move", Some(f), None) => Some(Change::delete(&f, c.is_dir)),
        // Moved in from outside: appears there.
        ("move", None, Some(t)) => Some(Change::create(&t, c.is_dir)),
        (_, Some(f), _) => Some(Change::new(&c.op, &f, None, c.is_dir)),
        ("delete" | "move", None, None) if swallows(&c.path) => Some(Change::delete("", true)),
        _ => None,
    }
}

/// Record changes inside a volume (paths relative to it). Raw volumes on the
/// same connection whose folders overlap get the change too, and their listing
/// cache for the touched folders is marked stale.
pub async fn record_volume(st: &AppState, volume_id: i64, changes: Vec<Change>) -> ApiResult<()> {
    if changes.is_empty() {
        return Ok(());
    }
    insert(st, Scope::Volume(volume_id), &changes).await?;
    let (conn, base, mode): (i64, String, String) =
        sqlx::query_as("SELECT connection_id, base_path, mode FROM volumes WHERE id = $1")
            .bind(volume_id)
            .fetch_one(&st.db)
            .await?;
    if mode != "raw" {
        return Ok(());
    }
    let others: Vec<(i64, String)> = sqlx::query_as(
        "SELECT id, base_path FROM volumes WHERE connection_id = $1 AND mode = 'raw' AND id <> $2",
    )
    .bind(conn)
    .bind(volume_id)
    .fetch_all(&st.db)
    .await?;
    for (other, obase) in others {
        let mapped: Vec<Change> = changes
            .iter()
            .filter_map(|c| map_into_overlap(&base, &obase, c))
            .collect();
        if mapped.is_empty() {
            continue;
        }
        if mapped.iter().any(|c| c.path.is_empty()) {
            sqlx::query("DELETE FROM raw_scans WHERE volume_id = $1")
                .bind(other)
                .execute(&st.db)
                .await?;
        } else {
            let dirs: Vec<String> = mapped
                .iter()
                .flat_map(|c| [Some(c.path.as_str()), c.new_path.as_deref()])
                .flatten()
                .map(|p| {
                    paths::split(p)
                        .map(|(d, _)| d.to_string())
                        .unwrap_or_default()
                })
                .collect();
            sqlx::query("DELETE FROM raw_scans WHERE volume_id = $1 AND dir = ANY($2)")
                .bind(other)
                .bind(&dirs)
                .execute(&st.db)
                .await?;
        }
        insert(st, Scope::Volume(other), &mapped).await?;
    }
    Ok(())
}

/// Other mounts of `user_id` that show an overlapping folder of the same
/// connection (one folder inside the other, or the same folder).
pub async fn overlapping_mounts(
    st: &AppState,
    user_id: i64,
    mount_id: i64,
) -> ApiResult<Vec<String>> {
    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT o.path, ov.base_path, mv.base_path FROM mounts m
           JOIN volumes mv ON mv.id = m.volume_id
           JOIN volumes ov ON ov.connection_id = mv.connection_id
           JOIN mounts o ON o.volume_id = ov.id AND o.owner_id = m.owner_id AND o.id <> m.id
         WHERE m.id = $1 AND m.owner_id = $2",
    )
    .bind(mount_id)
    .bind(user_id)
    .fetch_all(&st.db)
    .await?;
    Ok(rows
        .into_iter()
        .filter(|(_, a, b)| {
            paths::strip_prefix(a, b).is_some() || paths::strip_prefix(b, a).is_some()
        })
        .map(|(p, _, _)| p)
        .collect())
}

// ---------------------------------------------------------------- cursors

/// Opaque feed position: per-volume and user-scope sequence numbers.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Cursor {
    #[serde(default)]
    pub v: BTreeMap<i64, i64>,
    #[serde(default)]
    pub u: i64,
}

impl Cursor {
    pub fn encode(&self) -> String {
        hex::encode(serde_json::to_vec(self).unwrap())
    }
    pub fn decode(s: &str) -> ApiResult<Self> {
        let bytes = hex::decode(s).map_err(|_| bad("invalid cursor"))?;
        serde_json::from_slice(&bytes).map_err(|_| bad("invalid cursor"))
    }
}

/// The user's current position: everything up to now.
pub async fn current_cursor(st: &AppState, user_id: i64) -> ApiResult<Cursor> {
    let vols: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT DISTINCT v.id, v.journal_seq FROM mounts m JOIN volumes v ON v.id = m.volume_id
         WHERE m.owner_id = $1",
    )
    .bind(user_id)
    .fetch_all(&st.db)
    .await?;
    let u: i64 = sqlx::query_scalar("SELECT journal_seq FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_one(&st.db)
        .await?;
    Ok(Cursor {
        v: vols.into_iter().collect(),
        u,
    })
}

#[derive(Deserialize)]
pub struct ChangesQuery {
    pub cursor: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Serialize)]
pub struct FeedEntry {
    pub op: String,
    pub path: String,
    pub new_path: Option<String>,
    pub is_dir: bool,
    pub at: DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
struct Row {
    volume_id: Option<i64>,
    seq: i64,
    op: String,
    path: String,
    new_path: Option<String>,
    is_dir: bool,
    at: DateTime<Utc>,
}

pub enum Feed {
    Page {
        cursor: Cursor,
        more: bool,
        changes: Vec<FeedEntry>,
    },
    /// The cursor is older than the retained journal: list everything again.
    Resync,
}

/// Read a user's feed after `cursor` (at most `limit` journal rows).
pub async fn feed(st: &AppState, user_id: i64, cursor: Cursor, limit: i64) -> ApiResult<Feed> {
    let ms = mounts::user_mounts(st, user_id).await?;
    // Only volumes the user mounts count; anything else in the token is ignored.
    let mut after: BTreeMap<i64, i64> = BTreeMap::new();
    for m in ms.iter() {
        let from = cursor
            .v
            .get(&m.volume_id)
            .copied()
            .unwrap_or(m.journal_from);
        let e = after.entry(m.volume_id).or_insert(from);
        *e = (*e).min(from);
    }
    let vids: Vec<i64> = after.keys().copied().collect();
    let froms: Vec<i64> = after.values().copied().collect();
    let floors: Vec<(i64, i64)> =
        sqlx::query_as("SELECT id, journal_floor FROM volumes WHERE id = ANY($1)")
            .bind(&vids)
            .fetch_all(&st.db)
            .await?;
    let ufloor: i64 = sqlx::query_scalar("SELECT journal_floor FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_one(&st.db)
        .await?;
    if floors.iter().any(|(v, f)| after[v] < *f) || cursor.u < ufloor {
        return Ok(Feed::Resync);
    }
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT * FROM (
           SELECT j.id, j.volume_id, j.seq, j.op, j.path, j.new_path, j.is_dir, j.at
           FROM journal j JOIN unnest($1::int8[], $2::int8[]) AS c(vid, after)
             ON j.volume_id = c.vid AND j.seq > c.after
           UNION ALL
           SELECT j.id, j.volume_id, j.seq, j.op, j.path, j.new_path, j.is_dir, j.at
           FROM journal j WHERE j.user_id = $3 AND j.seq > $4
         ) f ORDER BY id LIMIT $5",
    )
    .bind(&vids)
    .bind(&froms)
    .bind(user_id)
    .bind(cursor.u)
    .bind(limit)
    .fetch_all(&st.db)
    .await?;
    let more = rows.len() as i64 == limit;
    let by_volume: HashMap<i64, Vec<&mounts::Mount>> =
        ms.iter().fold(HashMap::new(), |mut acc, m| {
            acc.entry(m.volume_id).or_default().push(m);
            acc
        });
    let mut next = Cursor {
        v: after,
        u: cursor.u,
    };
    let mut changes = Vec::new();
    for r in rows {
        match r.volume_id {
            Some(v) => {
                next.v.insert(v, r.seq);
                for m in by_volume.get(&v).into_iter().flatten() {
                    if r.seq <= m.journal_from {
                        continue;
                    }
                    let j = |p: &str| paths::join(&m.path, p);
                    changes.push(FeedEntry {
                        op: r.op.clone(),
                        path: j(&r.path),
                        new_path: r.new_path.as_deref().map(j),
                        is_dir: r.is_dir,
                        at: r.at,
                    });
                }
            }
            None => {
                next.u = r.seq;
                changes.push(FeedEntry {
                    op: r.op,
                    path: r.path,
                    new_path: r.new_path,
                    is_dir: r.is_dir,
                    at: r.at,
                });
            }
        }
    }
    Ok(Feed::Page {
        cursor: next,
        more,
        changes,
    })
}

/// `GET /api/changes?cursor=TOKEN`: changes after the cursor. Without a cursor
/// it returns the current position (list folders first, then follow it).
pub async fn changes(
    State(st): State<AppState>,
    AuthUser(u): AuthUser,
    Query(q): Query<ChangesQuery>,
) -> ApiResult<Response> {
    let Some(token) = q.cursor.filter(|c| !c.is_empty()) else {
        let c = current_cursor(&st, u.id).await?;
        return Ok(
            Json(json!({ "cursor": c.encode(), "more": false, "changes": [] })).into_response(),
        );
    };
    let limit = q.limit.unwrap_or(1000).clamp(1, 1000);
    match feed(&st, u.id, Cursor::decode(&token)?, limit).await? {
        Feed::Page {
            cursor,
            more,
            changes,
        } => Ok(
            Json(json!({ "cursor": cursor.encode(), "more": more, "changes": changes }))
                .into_response(),
        ),
        Feed::Resync => Ok((
            StatusCode::GONE,
            Json(json!({ "error": "cursor too old", "resync": true })),
        )
            .into_response()),
    }
}

/// Drop journal rows older than the retention window (30 days, or beyond the
/// newest 1M of a scope) and remember the newest pruned seq per scope.
pub async fn prune(st: &AppState) -> ApiResult<()> {
    sqlx::query(
        "WITH d AS (
           DELETE FROM journal j USING volumes v
           WHERE j.volume_id = v.id
             AND (j.at < now() - make_interval(days => $1) OR j.seq <= v.journal_seq - $2)
           RETURNING j.volume_id, j.seq)
         UPDATE volumes v SET journal_floor = GREATEST(v.journal_floor, x.m)
         FROM (SELECT volume_id, max(seq) AS m FROM d GROUP BY volume_id) x WHERE v.id = x.volume_id",
    )
    .bind(RETENTION_DAYS)
    .bind(RETENTION_ROWS)
    .execute(&st.db)
    .await?;
    sqlx::query(
        "WITH d AS (
           DELETE FROM journal j USING users u
           WHERE j.user_id = u.id
             AND (j.at < now() - make_interval(days => $1) OR j.seq <= u.journal_seq - $2)
           RETURNING j.user_id, j.seq)
         UPDATE users u SET journal_floor = GREATEST(u.journal_floor, x.m)
         FROM (SELECT user_id, max(seq) AS m FROM d GROUP BY user_id) x WHERE u.id = x.user_id",
    )
    .bind(RETENTION_DAYS)
    .bind(RETENTION_ROWS)
    .execute(&st.db)
    .await?;
    Ok(())
}

/// Forward `NOTIFY strata_journal` to this instance's websocket listeners,
/// and drop cached mount lists when a user's tree changed on any instance.
pub fn spawn_listener(st: AppState) {
    tokio::spawn(async move {
        let mut backoff = std::time::Duration::from_secs(1);
        loop {
            let res = async {
                let mut l = sqlx::postgres::PgListener::connect_with(&st.db).await?;
                l.listen(NOTIFY_CHANNEL).await?;
                let mut fresh = true;
                loop {
                    if fresh {
                        // (Re)connected: notifications may have been lost while
                        // we were away. Forget cached mounts, tell sockets.
                        crate::mounts::invalidate_all(&st);
                        let _ = st.events.send(ChangeEvent {
                            volume_id: None,
                            user_id: None,
                            connection_id: None,
                            audience: None,
                        });
                        backoff = std::time::Duration::from_secs(1);
                        fresh = false;
                    }
                    // `Ok(None)`: the connection dropped and the next call reconnects.
                    let Some(n) = l.try_recv().await? else {
                        fresh = true;
                        continue;
                    };
                    let v: serde_json::Value =
                        serde_json::from_str(n.payload()).unwrap_or_default();
                    let (vid, uid) = (v["v"].as_i64(), v["u"].as_i64());
                    if let Some(u) = uid {
                        mounts::invalidate(&st, u);
                    }
                    let _ = st.events.send(ChangeEvent {
                        volume_id: vid,
                        user_id: uid,
                        connection_id: None,
                        audience: None,
                    });
                }
                #[allow(unreachable_code)]
                Ok::<(), sqlx::Error>(())
            }
            .await;
            if let Err(e) = res {
                tracing::warn!("journal listener: {e}; retrying in {backoff:?}");
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(std::time::Duration::from_secs(60));
        }
    });
}

/// `GET /api/ws`: pushes `{"cursor": TOKEN}` whenever the user's feed grows.
pub async fn ws(
    State(st): State<AppState>,
    AuthUser(u): AuthUser,
    upgrade: WebSocketUpgrade,
) -> Response {
    upgrade.on_upgrade(move |socket| ws_loop(st, u.id, socket))
}

async fn relevant(st: &AppState, user_id: i64, ev: &ChangeEvent) -> bool {
    match (ev.user_id, ev.volume_id) {
        (Some(u), _) => u == user_id,
        // Listener reconnected: tell everyone to refresh.
        (None, None) => true,
        (None, Some(v)) => mounts::user_mounts(st, user_id)
            .await
            .map(|ms| ms.iter().any(|m| m.volume_id == v))
            .unwrap_or(false),
    }
}

async fn send_cursor(st: &AppState, user_id: i64, socket: &mut WebSocket) -> Result<(), ()> {
    let c = current_cursor(st, user_id).await.map_err(|_| ())?;
    let msg = json!({ "cursor": c.encode() }).to_string();
    socket.send(Message::Text(msg.into())).await.map_err(|_| ())
}

async fn ws_loop(st: AppState, user_id: i64, mut socket: WebSocket) {
    let mut rx = st.events.subscribe();
    if send_cursor(&st, user_id, &mut socket).await.is_err() {
        return;
    }
    let mut ping = tokio::time::interval(std::time::Duration::from_secs(30));
    // Bursts (a 1,000-file upload) are coalesced into one push per 250 ms.
    let mut pending = false;
    let flush = tokio::time::sleep(std::time::Duration::from_secs(3600));
    tokio::pin!(flush);
    loop {
        tokio::select! {
            ev = rx.recv() => match ev {
                Ok(ChangeEvent { connection_id: Some(c), audience, .. }) => {
                    // Health changes go out at once: `{"health": <connection id>}`.
                    if audience.is_some_and(|a| a.contains(&user_id)) {
                        let msg = json!({ "health": c }).to_string();
                        if socket.send(Message::Text(msg.into())).await.is_err() { return; }
                    }
                }
                Ok(ev) => {
                    if !pending && relevant(&st, user_id, &ev).await {
                        pending = true;
                        flush.as_mut().reset(tokio::time::Instant::now() + std::time::Duration::from_millis(250));
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    if !pending {
                        pending = true;
                        flush.as_mut().reset(tokio::time::Instant::now() + std::time::Duration::from_millis(250));
                    }
                }
                Err(_) => return,
            },
            _ = &mut flush, if pending => {
                pending = false;
                if send_cursor(&st, user_id, &mut socket).await.is_err() { return; }
            }
            msg = socket.recv() => match msg {
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return,
                _ => {}
            },
            _ = ping.tick() => {
                if socket.send(Message::Ping(Vec::new().into())).await.is_err() { return; }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlap_mapping() {
        let c = Change::create("photos/a.jpg", false);
        assert_eq!(
            map_into_overlap("", "photos", &c),
            Some(Change::create("a.jpg", false))
        );
        assert_eq!(map_into_overlap("", "docs", &c), None);
        assert_eq!(
            map_into_overlap("photos", "", &Change::create("a", false)),
            Some(Change::create("photos/a", false))
        );
        // moves across the boundary
        let m = Change::moved("x/f", "y/f", false);
        assert_eq!(
            map_into_overlap("", "x", &m),
            Some(Change::delete("f", false))
        );
        assert_eq!(
            map_into_overlap("", "y", &m),
            Some(Change::create("f", false))
        );
        // the other volume's folder is inside what was deleted
        assert_eq!(
            map_into_overlap("", "a/b", &Change::delete("a", true)),
            Some(Change::delete("", true))
        );
        assert_eq!(
            map_into_overlap("", "a/b", &Change::moved("a", "c", true)),
            Some(Change::delete("", true))
        );
    }

    #[test]
    fn cursor_roundtrip() {
        let c = Cursor {
            v: [(1, 5), (9, 2)].into_iter().collect(),
            u: 3,
        };
        assert_eq!(Cursor::decode(&c.encode()).unwrap(), c);
        assert!(Cursor::decode("zz").is_err());
    }
}
