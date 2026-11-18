//! The virtual file system: a user's tree stitched together from mounts.
//!
//! * HASHED volumes keep their tree only in `nodes`; file content is a list
//!   of blobs in the volume's content-addressed store.
//! * RAW volumes mirror backend folders; `nodes` is a listing cache refreshed
//!   by scanning (on access after `raw_ttl_secs`, or a full rescan).

use std::io;
use std::pin::Pin;

use bytes::Bytes;
use chrono::{DateTime, Utc};
use futures::{Stream, StreamExt};
use opendal::{ErrorKind, Operator};
use serde::Serialize;
use strata_storage::chunker::StreamChunker;
use strata_storage::hashed::manifest_hash;
use strata_storage::{Hash, HashedStore, paths};

use crate::error::{ApiError, ApiResult, bad, conflict};
use crate::journal::{self, Change};
use crate::mounts::{self, Mount};
use crate::state::AppState;

pub type ByteStream = Pin<Box<dyn Stream<Item = io::Result<Bytes>> + Send>>;

#[derive(Clone, Debug, Serialize, sqlx::FromRow)]
pub struct Entry {
    pub name: String,
    pub is_dir: bool,
    pub size: i64,
    pub mtime: DateTime<Utc>,
    pub etag: Option<String>,
    /// Set when this entry is a mount point.
    pub mount_id: Option<i64>,
    /// A mount point whose storage is offline (it stays in the listing).
    pub offline: bool,
}

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct Node {
    pub name: String,
    pub is_dir: bool,
    pub size: i64,
    pub mtime: DateTime<Utc>,
    pub etag: Option<String>,
    pub chunks: Option<Vec<Vec<u8>>>,
    pub chunk_sizes: Option<Vec<i64>>,
}

/// Escape a string for use as a LIKE prefix.
fn like_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn backend_path(m: &Mount, rel: &str, is_dir: bool) -> String {
    let p = paths::join(&m.base_path, rel);
    match (p.is_empty(), is_dir) {
        (true, _) => "/".into(),
        (false, true) => format!("{p}/"),
        (false, false) => p,
    }
}

/// Serialises tree changes of one volume (across server instances), so the
/// journal records them in the order they happened. Held as a transaction
/// with a transaction-level advisory lock: dropping it releases the lock.
///
/// Waiters first queue on an in-process mutex, so at most one task per volume
/// and instance holds a database connection while waiting for the advisory
/// lock (otherwise many waiters could exhaust the pool and starve the holder).
pub struct VolumeLock(
    sqlx::Transaction<'static, sqlx::Postgres>,
    #[allow(dead_code)]
    (
        Vec<tokio::sync::OwnedMutexGuard<()>>,
        tokio::sync::OwnedSemaphorePermit,
    ), // held until release/drop
);

pub async fn lock_volume(st: &AppState, volume_id: i64) -> ApiResult<VolumeLock> {
    lock_volumes(st, &[volume_id]).await
}

/// Lock several volumes at once (always in ascending id order, so two
/// multi-locks can't deadlock; single locks are a special case of it).
/// One permit and one transaction cover them all.
pub async fn lock_volumes(st: &AppState, ids: &[i64]) -> ApiResult<VolumeLock> {
    let mut ids = ids.to_vec();
    ids.sort_unstable();
    ids.dedup();
    let mut guards = Vec::with_capacity(ids.len());
    for id in &ids {
        let local = st.volume_locks.entry(*id).or_default().clone();
        guards.push(local.lock_owned().await);
    }
    // Cap concurrent holders well below the pool size, so every holder can
    // still get the extra connections its locked work needs.
    let permit = st
        .lock_permits
        .clone()
        .acquire_owned()
        .await
        .map_err(|e| ApiError::Internal(e.into()))?;
    let mut tx = st.db.begin().await?;
    for id in &ids {
        sqlx::query("SELECT pg_advisory_xact_lock(1398035009, $1::int4)")
            .bind(*id as i32)
            .execute(&mut *tx)
            .await?;
    }
    Ok(VolumeLock(tx, (guards, permit)))
}

impl VolumeLock {
    pub async fn release(self) -> ApiResult<()> {
        self.0.commit().await?;
        Ok(())
    }
}

/// A raw local mount never exposes `.strata-tmp` folders (where atomic writes
/// are staged), at any depth; no local connection or mount may point into
/// one either. No raw mount exposes a `.strata-probe` folder (the health
/// probes' folder, at any connection's root). The server's own state lives outside the
/// storage base.
pub fn is_reserved(m: &Mount, rel: &str) -> bool {
    if m.is_hashed() {
        return false;
    }
    let full = paths::join(&m.base_path, rel);
    // The health probes' folder, on any backend and at any depth (another
    // connection's root, with its marker, can be inside this one).
    full.split('/')
        .any(|c| c == strata_storage::backend::PROBE_DIR)
        || (m.connection_kind == "local" && strata_storage::backend::has_reserved_component(&full))
}

/// Changes need the mount's storage online, including ones kept only in the
/// database (folders, moves and deletes in a hashed mount): an offline
/// mount is read-only at best until its storage is back.
pub fn ensure_writable(st: &AppState, m: &Mount) -> ApiResult<()> {
    st.ensure_online(m.connection_id)
}

pub async fn hashed_store(st: &AppState, m: &Mount) -> ApiResult<HashedStore> {
    Ok(HashedStore::new(
        st.operator(m.connection_id).await?,
        &m.base_path,
    ))
}

async fn op(st: &AppState, m: &Mount) -> ApiResult<Operator> {
    st.operator(m.connection_id).await
}

// ---------------------------------------------------------------- lookups

/// Metadata of one entry of a volume (no scanning).
pub async fn get_node(st: &AppState, volume_id: i64, rel: &str) -> ApiResult<Option<Node>> {
    let Some((parent, name)) = paths::split(rel) else {
        return Ok(Some(Node {
            name: String::new(),
            is_dir: true,
            size: 0,
            mtime: Utc::now(),
            etag: None,
            chunks: None,
            chunk_sizes: None,
        }));
    };
    Ok(sqlx::query_as(
        "SELECT name, is_dir, size, mtime, etag, chunks, chunk_sizes FROM nodes
         WHERE volume_id = $1 AND parent = $2 AND name = $3",
    )
    .bind(volume_id)
    .bind(parent)
    .bind(name)
    .fetch_optional(&st.db)
    .await?)
}

/// Database id of a volume entry (None for the volume root or unknown paths).
pub async fn node_id(st: &AppState, volume_id: i64, rel: &str) -> ApiResult<Option<i64>> {
    let Some((parent, name)) = paths::split(rel) else {
        return Ok(None);
    };
    Ok(sqlx::query_scalar(
        "SELECT id FROM nodes WHERE volume_id = $1 AND parent = $2 AND name = $3",
    )
    .bind(volume_id)
    .bind(parent)
    .bind(name)
    .fetch_optional(&st.db)
    .await?)
}

/// Stat a path of a mount. Raw volumes are asked directly.
pub async fn stat(st: &AppState, m: &Mount, rel: &str) -> ApiResult<Node> {
    if m.is_hashed() || rel.is_empty() {
        return get_node(st, m.volume_id, rel)
            .await?
            .ok_or(ApiError::NotFound);
    }
    let op = op(st, m).await?;
    let meta = match op.stat(&backend_path(m, rel, false)).await {
        Ok(meta) => meta,
        Err(e) if e.kind() == ErrorKind::NotFound => {
            // Might be a folder on backends that need the trailing slash.
            op.stat(&backend_path(m, rel, true)).await?
        }
        Err(e) => return Err(e.into()),
    };
    let node = Node {
        name: paths::split(rel).unwrap().1.to_string(),
        is_dir: meta.is_dir(),
        size: meta.content_length() as i64,
        mtime: meta_mtime(&meta),
        etag: meta
            .etag()
            .map(str::to_string)
            .or_else(|| Some(raw_etag(&meta))),
        chunks: None,
        chunk_sizes: None,
    };
    // A pure read: the cache is only written under the volume lock.
    Ok(node)
}

fn meta_mtime(meta: &opendal::Metadata) -> DateTime<Utc> {
    meta.last_modified()
        .and_then(|t| {
            DateTime::<Utc>::from_timestamp_nanos(t.into_inner().as_nanosecond() as i64).into()
        })
        .unwrap_or_else(Utc::now)
}

/// Our own etag for raw entries whose backend gives none:
/// `<size hex>-<mtime ns hex>`.
fn raw_etag(meta: &opendal::Metadata) -> String {
    format!(
        "{:x}-{:x}",
        meta.content_length(),
        meta.last_modified()
            .map(|t| t.into_inner().as_nanosecond())
            .unwrap_or(0)
    )
}

// ---------------------------------------------------------------- listing

/// List a folder of the user's tree. For a fresh folder this is one query.
pub async fn list(st: &AppState, user_id: i64, path: &str) -> ApiResult<(Mount, Vec<Entry>)> {
    let (m, rel) = mounts::resolve(st, user_id, path).await?;
    if !m.is_hashed() {
        maybe_scan(st, &m, &rel).await?;
    }
    let entries: Vec<Entry> = sqlx::query_as(
        "SELECT n.name, n.is_dir, n.size, n.mtime, n.etag, NULL::bigint AS mount_id, FALSE AS offline
         FROM nodes n
         WHERE n.volume_id = $1 AND n.parent = $2
           AND NOT EXISTS (SELECT 1 FROM mounts x WHERE x.owner_id = $3 AND x.parent_path = $4 AND x.name = n.name)
         UNION ALL
         SELECT x.name, TRUE, 0, x.created_at, NULL, x.id, c.health = 'offline'
         FROM mounts x JOIN volumes v ON v.id = x.volume_id JOIN connections c ON c.id = v.connection_id
         WHERE x.owner_id = $3 AND x.parent_path = $4
         ORDER BY 2 DESC, 1",
    )
    .bind(m.volume_id)
    .bind(&rel)
    .bind(user_id)
    .bind(path)
    .fetch_all(&st.db)
    .await?;
    if entries.is_empty() && !rel.is_empty() {
        // Distinguish "empty folder" from "missing" / "a file".
        match get_node(st, m.volume_id, &rel).await? {
            Some(n) if n.is_dir => {}
            Some(_) => return Err(bad("not a folder")),
            None => return Err(ApiError::NotFound),
        }
    }
    Ok((m, entries))
}

/// Rescan a raw folder if its cached listing is older than the TTL.
pub async fn maybe_scan(st: &AppState, m: &Mount, rel: &str) -> ApiResult<()> {
    let fresh: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM raw_scans WHERE volume_id = $1 AND dir = $2
                        AND scanned_at > now() - make_interval(secs => $3))",
    )
    .bind(m.volume_id)
    .bind(rel)
    .bind(st.cfg.raw_ttl_secs as f64)
    .fetch_one(&st.db)
    .await?;
    if !fresh {
        scan_dir_inner(st, m, rel, true).await?;
    }
    Ok(())
}

/// List a raw folder from the backend and reconcile the cache. Returns the
/// sub-folder names.
pub async fn scan_dir(st: &AppState, m: &Mount, rel: &str) -> ApiResult<Vec<String>> {
    scan_dir_inner(st, m, rel, false).await
}

async fn scan_dir_inner(
    st: &AppState,
    m: &Mount,
    rel: &str,
    only_if_stale: bool,
) -> ApiResult<Vec<String>> {
    // DB clock, the same one that stamps raw_scans.
    let asked_at: DateTime<Utc> = sqlx::query_scalar("SELECT now()").fetch_one(&st.db).await?;
    // Hold the volume lock from listing to reconciling, so no write or delete
    // can land between our snapshot of the backend and the cache update.
    let lock = lock_volume(st, m.volume_id).await?;
    if only_if_stale {
        // Someone may have rescanned while we waited for the lock.
        let fresh: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM raw_scans WHERE volume_id = $1 AND dir = $2 AND scanned_at > $3)",
        )
        .bind(m.volume_id)
        .bind(rel)
        .bind(asked_at)
        .fetch_one(&st.db)
        .await?;
        if fresh {
            lock.release().await?;
            return Ok(Vec::new());
        }
    }
    let op = op(st, m).await?;
    let dir = backend_path(m, rel, true);
    let listed = match op.list(&dir).await {
        Ok(l) => l,
        Err(e) if e.kind() == ErrorKind::NotFound && rel.is_empty() => {
            op.create_dir(&dir).await?;
            Vec::new()
        }
        Err(e) => return Err(e.into()),
    };
    let dir_trim = dir.trim_start_matches('/');
    let mut nodes = Vec::new();
    let mut need_stat = Vec::new();
    for e in listed {
        let p = e.path().trim_start_matches('/');
        if p == dir_trim || p.is_empty() {
            continue;
        }
        let name = e.name().trim_end_matches('/').to_string();
        if paths::validate_name(&name).is_err() || is_reserved(m, &paths::join(rel, &name)) {
            continue;
        }
        let meta = e.metadata();
        let is_dir = meta.is_dir();
        if !is_dir && meta.last_modified().is_none() {
            need_stat.push(name);
            continue;
        }
        nodes.push(Node {
            name,
            is_dir,
            size: if is_dir {
                0
            } else {
                meta.content_length() as i64
            },
            mtime: meta_mtime(meta),
            etag: if is_dir {
                None
            } else {
                Some(
                    meta.etag()
                        .map(str::to_string)
                        .unwrap_or_else(|| raw_etag(meta)),
                )
            },
            chunks: None,
            chunk_sizes: None,
        });
    }
    // Some backends (local fs) list names only; stat those concurrently.
    // Every name the backend listed, even if its stat failed below: such an
    // entry is still there and must not be treated as deleted.
    let mut listed_names: std::collections::HashSet<String> =
        nodes.iter().map(|n| n.name.clone()).collect();
    listed_names.extend(need_stat.iter().cloned());
    let stated: Vec<Option<Node>> = futures::stream::iter(need_stat)
        .map(|name| {
            let op = op.clone();
            let p = backend_path(m, &paths::join(rel, &name), false);
            async move {
                let meta = op.stat(&p).await.ok()?;
                Some(Node {
                    is_dir: meta.is_dir(),
                    size: meta.content_length() as i64,
                    mtime: meta_mtime(&meta),
                    etag: Some(
                        meta.etag()
                            .map(str::to_string)
                            .unwrap_or_else(|| raw_etag(&meta)),
                    ),
                    name,
                    chunks: None,
                    chunk_sizes: None,
                })
            }
        })
        .buffer_unordered(16)
        .collect()
        .await;
    nodes.extend(stated.into_iter().flatten());

    // Reconcile with the cache.
    let was_scanned: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM raw_scans WHERE volume_id = $1 AND dir = $2)",
    )
    .bind(m.volume_id)
    .bind(rel)
    .fetch_one(&st.db)
    .await?;
    let old: Vec<(String, bool, Option<String>)> =
        sqlx::query_as("SELECT name, is_dir, etag FROM nodes WHERE volume_id = $1 AND parent = $2")
            .bind(m.volume_id)
            .bind(rel)
            .fetch_all(&st.db)
            .await?;
    let old_map: std::collections::HashMap<&str, (bool, Option<&str>)> = old
        .iter()
        .map(|(n, d, e)| (n.as_str(), (*d, e.as_deref())))
        .collect();
    let mut changes = Vec::new();
    for n in &nodes {
        let p = paths::join(rel, &n.name);
        match old_map.get(n.name.as_str()) {
            None => changes.push(Change::create(&p, n.is_dir)),
            Some((d, _)) if *d != n.is_dir => {
                changes.push(Change::delete(&p, *d));
                changes.push(Change::create(&p, n.is_dir));
            }
            Some((false, e)) if *e != n.etag.as_deref() => changes.push(Change::modify(&p)),
            _ => {}
        }
    }
    let gone: Vec<&(String, bool, Option<String>)> = old
        .iter()
        .filter(|(n, _, _)| !listed_names.contains(n.as_str()))
        .collect();
    for (name, is_dir, _) in &gone {
        let p = paths::join(rel, name);
        delete_subtree_rows(st, m.volume_id, &p).await?;
        changes.push(Change::delete(&p, *is_dir));
    }
    // Entries whose type changed: drop the old subtree first.
    for n in &nodes {
        if let Some((d, _)) = old_map.get(n.name.as_str())
            && *d
            && !n.is_dir
        {
            delete_subtree_rows(st, m.volume_id, &paths::join(rel, &n.name)).await?;
        }
    }
    upsert_raw_nodes(st, m.volume_id, rel, &nodes).await?;
    sqlx::query(
        "INSERT INTO raw_scans (volume_id, dir, scanned_at) VALUES ($1, $2, now())
         ON CONFLICT (volume_id, dir) DO UPDATE SET scanned_at = now()",
    )
    .bind(m.volume_id)
    .bind(rel)
    .execute(&st.db)
    .await?;
    if was_scanned {
        journal::record_volume(st, m.volume_id, changes).await?;
    }
    lock.release().await?;
    Ok(nodes
        .into_iter()
        .filter(|n| n.is_dir)
        .map(|n| n.name)
        .collect())
}

/// Recursively scan a raw mount. Returns the number of folders scanned.
pub async fn scan_tree(st: &AppState, m: &Mount, rel: &str) -> ApiResult<usize> {
    let mut stack = vec![rel.to_string()];
    let mut count = 0;
    while let Some(dir) = stack.pop() {
        let subdirs = scan_dir(st, m, &dir).await?;
        count += 1;
        stack.extend(subdirs.into_iter().map(|s| paths::join(&dir, &s)));
    }
    Ok(count)
}

/// Put one raw entry into the cache (caller holds the volume lock). Returns
/// true if the row is new.
async fn cache_node(st: &AppState, m: &Mount, rel: &str, n: &Node) -> ApiResult<bool> {
    let (parent, name) = paths::split(rel).ok_or_else(|| bad("not an entry"))?;
    Ok(sqlx::query_scalar(
        "INSERT INTO nodes (volume_id, parent, name, is_dir, size, mtime, etag)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         ON CONFLICT (volume_id, parent, name) DO UPDATE
           SET is_dir = EXCLUDED.is_dir, size = EXCLUDED.size, mtime = EXCLUDED.mtime, etag = EXCLUDED.etag
         RETURNING (xmax = 0)",
    )
    .bind(m.volume_id)
    .bind(parent)
    .bind(name)
    .bind(n.is_dir)
    .bind(n.size)
    .bind(n.mtime)
    .bind(&n.etag)
    .fetch_one(&st.db)
    .await?)
}

/// Make sure a raw entry has a cache row (e.g. before sharing it). No-op for
/// hashed volumes, whose rows are the truth.
pub async fn ensure_cached(st: &AppState, m: &Mount, rel: &str) -> ApiResult<()> {
    if m.is_hashed() || rel.is_empty() {
        return Ok(());
    }
    let lock = lock_volume(st, m.volume_id).await?;
    let n = stat(st, m, rel).await?;
    cache_node(st, m, rel, &n).await?;
    lock.release().await
}

async fn upsert_raw_nodes(
    st: &AppState,
    volume_id: i64,
    parent: &str,
    nodes: &[Node],
) -> ApiResult<()> {
    if nodes.is_empty() {
        return Ok(());
    }
    let names: Vec<&str> = nodes.iter().map(|n| n.name.as_str()).collect();
    let dirs: Vec<bool> = nodes.iter().map(|n| n.is_dir).collect();
    let sizes: Vec<i64> = nodes.iter().map(|n| n.size).collect();
    let mtimes: Vec<DateTime<Utc>> = nodes.iter().map(|n| n.mtime).collect();
    let etags: Vec<Option<String>> = nodes.iter().map(|n| n.etag.clone()).collect();
    sqlx::query(
        "INSERT INTO nodes (volume_id, parent, name, is_dir, size, mtime, etag)
         SELECT $1, $2, * FROM unnest($3::text[], $4::bool[], $5::int8[], $6::timestamptz[], $7::text[])
         ON CONFLICT (volume_id, parent, name) DO UPDATE
           SET is_dir = EXCLUDED.is_dir, size = EXCLUDED.size, mtime = EXCLUDED.mtime, etag = EXCLUDED.etag",
    )
    .bind(volume_id)
    .bind(parent)
    .bind(names)
    .bind(dirs)
    .bind(sizes)
    .bind(mtimes)
    .bind(etags)
    .execute(&st.db)
    .await?;
    Ok(())
}

/// Delete a node and everything below it from `nodes` (and raw scan marks).
async fn delete_subtree_rows(st: &AppState, volume_id: i64, rel: &str) -> ApiResult<()> {
    let (parent, name) = paths::split(rel).ok_or_else(|| bad("cannot delete the root"))?;
    let like = format!("{}/%", like_escape(rel));
    sqlx::query(
        "DELETE FROM nodes WHERE volume_id = $1 AND ((parent = $2 AND name = $3) OR parent = $4 OR parent LIKE $5)",
    )
    .bind(volume_id)
    .bind(parent)
    .bind(name)
    .bind(rel)
    .bind(&like)
    .execute(&st.db)
    .await?;
    sqlx::query("DELETE FROM raw_scans WHERE volume_id = $1 AND (dir = $2 OR dir LIKE $3)")
        .bind(volume_id)
        .bind(rel)
        .bind(&like)
        .execute(&st.db)
        .await?;
    Ok(())
}

// ---------------------------------------------------------------- mkdir

/// Create a folder (and missing parents) in the user's tree.
pub async fn mkdir_p(st: &AppState, user_id: i64, path: &str) -> ApiResult<()> {
    if path.is_empty() {
        return Ok(());
    }
    let (m, rel) = mounts::resolve(st, user_id, path).await?;
    if rel.is_empty() {
        return Ok(()); // a mount point
    }
    mkdir_in(st, &m, &rel).await
}

pub async fn mkdir_in(st: &AppState, m: &Mount, rel: &str) -> ApiResult<()> {
    ensure_writable(st, m)?;
    // Ancestors including rel itself.
    let parts: Vec<&str> = rel.split('/').collect();
    let mut parents = Vec::new();
    let mut names = Vec::new();
    for i in 0..parts.len() {
        parents.push(parts[..i].join("/"));
        names.push(parts[i].to_string());
    }
    let lock = lock_volume(st, m.volume_id).await?;
    // Refuse before touching anything if a path component is a file.
    let files: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM nodes n JOIN unnest($2::text[], $3::text[]) AS a(parent, name)
           ON n.parent = a.parent AND n.name = a.name
         WHERE n.volume_id = $1 AND NOT n.is_dir",
    )
    .bind(m.volume_id)
    .bind(&parents)
    .bind(&names)
    .fetch_one(&st.db)
    .await?;
    if files > 0 {
        return Err(conflict("a file with that name already exists"));
    }
    if !m.is_hashed() {
        op(st, m)
            .await?
            .create_dir(&backend_path(m, rel, true))
            .await?;
    }
    let created: Vec<(String, String)> = sqlx::query_as(
        "INSERT INTO nodes (volume_id, parent, name, is_dir)
         SELECT $1, * , TRUE FROM unnest($2::text[], $3::text[])
         ON CONFLICT (volume_id, parent, name) DO NOTHING RETURNING parent, name",
    )
    .bind(m.volume_id)
    .bind(&parents)
    .bind(&names)
    .fetch_all(&st.db)
    .await?;
    let changes = created
        .iter()
        .map(|(p, n)| Change::create(&paths::join(p, n), true))
        .collect();
    journal::record_volume(st, m.volume_id, changes).await?;
    lock.release().await
}

// ---------------------------------------------------------------- reading

/// Stream `range` (start..end, end exclusive) of a file.
pub async fn read_stream(
    st: &AppState,
    m: &Mount,
    rel: &str,
    node: &Node,
    start: u64,
    end: u64,
) -> ApiResult<ByteStream> {
    if m.is_hashed() {
        let store = hashed_store(st, m).await?;
        let chunks = node.chunks.clone().unwrap_or_default();
        let sizes = node.chunk_sizes.clone().unwrap_or_default();
        let s = async_stream::try_stream! {
            let mut off = 0u64;
            for (c, sz) in chunks.iter().zip(sizes.iter()) {
                let sz = *sz as u64;
                let (cs, ce) = (off, off + sz);
                off = ce;
                if ce <= start || cs >= end { continue; }
                let h = Hash::from_slice(c).ok_or_else(|| io::Error::other("bad chunk hash"))?;
                let a = start.saturating_sub(cs);
                let b = end.min(ce) - cs;
                let data = if a == 0 && b == sz {
                    store.get(&h).await
                } else {
                    store.get_range(&h, a..b).await
                }.map_err(io::Error::other)?;
                yield data;
            }
        };
        Ok(Box::pin(s))
    } else {
        let op = op(st, m).await?;
        if start >= end {
            return Ok(Box::pin(futures::stream::empty()));
        }
        let reader = op
            .reader_with(&backend_path(m, rel, false))
            .chunk(4 << 20)
            .await?;
        let s = reader.into_bytes_stream(start..end).await?;
        Ok(Box::pin(s))
    }
}

/// BLAKE3 of a file's full content (used to verify copies).
pub async fn content_digest(st: &AppState, m: &Mount, rel: &str) -> ApiResult<Hash> {
    let node = stat(st, m, rel).await?;
    digest_node(st, m, rel, &node).await
}

// ---------------------------------------------------------------- writing

/// Make sure blob `h` is in a hashed volume's store, writing `data` only if
/// needed. Safe against GC, which works in two steps (mark `deleting`, then
/// delete the object and the row under the row lock):
///
/// * a `blobs` row is only inserted after its object was written, so a row
///   that is not `deleting` always has its object;
/// * the row is locked while we look at it, so GC cannot delete the object in
///   between; a `deleting` row (GC crashed or is about to delete) gets its
///   object written again and the flag cleared, and GC re-checks the flag
///   under the lock before deleting anything.
pub async fn store_blob(
    st: &AppState,
    store: &HashedStore,
    volume_id: i64,
    h: &Hash,
    data: Bytes,
) -> ApiResult<()> {
    let mut tx = st.db.begin().await?;
    let row: Option<bool> = sqlx::query_scalar(
        "SELECT deleting FROM blobs WHERE volume_id = $1 AND hash = $2 FOR UPDATE",
    )
    .bind(volume_id)
    .bind(h.0.to_vec())
    .fetch_optional(&mut *tx)
    .await?;
    match row {
        Some(false) => {
            sqlx::query("UPDATE blobs SET created_at = now() WHERE volume_id = $1 AND hash = $2")
                .bind(volume_id)
                .bind(h.0.to_vec())
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
        }
        Some(true) => {
            store.put_overwrite(h, data).await?;
            sqlx::query(
                "UPDATE blobs SET deleting = FALSE, created_at = now() WHERE volume_id = $1 AND hash = $2",
            )
            .bind(volume_id)
            .bind(h.0.to_vec())
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
        }
        None => {
            tx.commit().await?;
            let size = data.len() as i64;
            store.put(h, data).await?;
            sqlx::query(
                "INSERT INTO blobs (volume_id, hash, size) VALUES ($1, $2, $3)
                 ON CONFLICT (volume_id, hash) DO UPDATE SET created_at = now(), deleting = FALSE",
            )
            .bind(volume_id)
            .bind(h.0.to_vec())
            .bind(size)
            .execute(&st.db)
            .await?;
        }
    }
    Ok(())
}

/// Refresh blobs that are about to be referenced without being re-sent (a
/// same-volume copy, chunks a user already stored). Returns the hashes that
/// are *not* safely present: missing, or being deleted by GC.
pub async fn touch_blobs(
    st: &AppState,
    volume_id: i64,
    hashes: &[Vec<u8>],
) -> ApiResult<Vec<Vec<u8>>> {
    let ok: std::collections::HashSet<Vec<u8>> = sqlx::query_scalar(
        "UPDATE blobs SET created_at = now()
         WHERE volume_id = $1 AND hash = ANY($2) AND NOT deleting RETURNING hash",
    )
    .bind(volume_id)
    .bind(hashes)
    .fetch_all(&st.db)
    .await?
    .into_iter()
    .collect();
    Ok(hashes
        .iter()
        .filter(|h| !ok.contains(*h))
        .cloned()
        .collect())
}

/// A file version's identity, the same for every read and write path:
/// hashed volumes use the content hash; raw volumes a hash of the size and
/// the backend's etag (the backend's own, or size plus mtime in nanoseconds
/// where it has none). It never depends on the request time.
///
/// Raw metadata can be too coarse to see every change (whole-second mtimes,
/// backend etags built from them). Saves from the editor therefore also send
/// the BLAKE3 of the bytes they were based on, and the server compares it
/// with the file's content under the lock (`Expect::Etag::base`).
pub fn version(node: &Node) -> String {
    match (&node.chunks, &node.etag) {
        (Some(_), Some(e)) => e.clone(),
        _ => {
            let s = format!("{}|{}", node.size, node.etag.as_deref().unwrap_or(""));
            blake3::hash(s.as_bytes()).to_hex()[..32].to_string()
        }
    }
}

/// HTTP ETag for a file version (content responses and writes). The `r3`
/// prefix is the serving-rules version: bump it when the way file bytes are
/// served changes, so cached responses don't revalidate into place.
pub fn etag_for(node: &Node) -> String {
    format!("\"r3-{}\"", version(node))
}

/// Largest file whose content is compared on a conditional raw save.
pub const BASE_CHECK_MAX: i64 = 5 * 1024 * 1024;

/// What a write expects to find at the target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expect {
    /// Overwrite or create unconditionally (uploads, copies).
    Any,
    /// The file must exist with this ETag (as from `etag_for`) and, if
    /// `base` is given, with exactly this content (BLAKE3 of its bytes).
    Etag { etag: String, base: Option<Hash> },
    /// The file must not exist (`If-None-Match: *`).
    Absent,
}

/// Check a precondition against the current state (call under the volume
/// lock, so nothing can change between the check and the write). Metadata
/// only; raw saves also compare content (`verify_raw_base`).
pub fn check_expect(expect: &Expect, current: Option<&Node>) -> ApiResult<()> {
    if let Some(n) = current
        && n.is_dir
    {
        return Err(conflict("a folder with that name already exists"));
    }
    match (expect, current) {
        (Expect::Any, _) | (Expect::Absent, None) => Ok(()),
        (Expect::Etag { etag, .. }, Some(n)) if *etag == etag_for(n) => Ok(()),
        (_, cur) => Err(ApiError::PreconditionFailed(cur.map(etag_for))),
    }
}

/// A raw entry whose version can miss a change: no backend etag and an
/// mtime of whole seconds at best, so a same-size rewrite within the same
/// second keeps its version. Such files are never answered with 304 and
/// their type isn't cached; saves are protected by the base-content check.
pub fn is_coarse(node: &Node) -> bool {
    if node.chunks.is_some() {
        return false;
    }
    let Some(tag) = node.etag.as_deref() else {
        return true;
    };
    let tag = tag.trim_start_matches("W/").trim_matches('"');
    let size = format!("{:x}", node.size);
    match tag.split_once('-') {
        // Ours (`raw_etag`): size, then mtime in ns; coarse when whole seconds.
        Some((s, ns)) if s == size => {
            u128::from_str_radix(ns, 16).is_ok_and(|ns| ns % 1_000_000_000 == 0)
        }
        // `mtime-size` style backend etags (nginx and friends): second resolution.
        Some((_, s)) if s == size => true,
        _ => false,
    }
}

/// BLAKE3 of a file's content, read from an already-stat'ed node.
async fn digest_node(st: &AppState, m: &Mount, rel: &str, node: &Node) -> ApiResult<Hash> {
    let mut s = read_stream(st, m, rel, node, 0, node.size as u64).await?;
    let mut h = blake3::Hasher::new();
    while let Some(b) = s.next().await {
        h.update(&b.map_err(|e| ApiError::Internal(e.into()))?);
    }
    Ok(Hash(*h.finalize().as_bytes()))
}

/// Cheap checks for a raw save, before taking the lock: the precondition
/// against metadata, and that a raw `If-Match` carries a base hash (raw
/// metadata can be too coarse to see every change, so one without is
/// refused with 428 rather than trusted). The content itself is compared
/// once, under the lock (`recheck_raw_base`). Returns the version seen.
async fn verify_raw_base(
    st: &AppState,
    m: &Mount,
    rel: &str,
    expect: &Expect,
) -> ApiResult<Option<Node>> {
    let current = current_file(st, m, rel).await?;
    check_expect(expect, current.as_ref())?;
    if let (Expect::Etag { base, .. }, Some(n)) = (expect, &current) {
        if base.is_none() {
            return Err(ApiError::PreconditionRequired(
                "saving over a file on this storage also needs X-Strata-Base-Hash",
            ));
        }
        // An edit can only be based on a file the editor could save
        // (BASE_CHECK_MAX); anything bigger has changed by definition.
        if n.size > BASE_CHECK_MAX {
            return Err(ApiError::PreconditionFailed(Some(etag_for(n))));
        }
    }
    Ok(current)
}

/// Under the lock: the version must still be the one `verify_raw_base`
/// checked, and the content still the base. Metadata alone can miss a
/// same-size rewrite within the backend's timestamp granularity (a second,
/// or a kernel tick), so the content is hashed again (at most 5 MB).
async fn recheck_raw_base(
    st: &AppState,
    m: &Mount,
    rel: &str,
    expect: &Expect,
    verified: &Option<Node>,
) -> ApiResult<()> {
    let now = current_file(st, m, rel).await?;
    check_expect(expect, now.as_ref())?;
    if let (Some(a), Some(b)) = (verified, &now) {
        let same = etag_for(a) == etag_for(b);
        if !same {
            return Err(ApiError::PreconditionFailed(Some(etag_for(b))));
        }
        if let Expect::Etag {
            base: Some(base), ..
        } = expect
            && digest_node(st, m, rel, b).await? != *base
        {
            return Err(ApiError::PreconditionFailed(Some(etag_for(b))));
        }
    }
    Ok(())
}

/// Lock every raw volume overlapping this one. The set is checked again
/// under the lock: a volume created (or re-pointed) onto the same folder
/// meanwhile is locked too.
async fn lock_overlapping_raw(st: &AppState, m: &Mount) -> ApiResult<VolumeLock> {
    let mut ids = overlapping_raw_volumes(st, m).await?;
    for _ in 0..3 {
        let l = lock_volumes(st, &ids).await?;
        let now = overlapping_raw_volumes(st, m).await?;
        if now.iter().all(|v| ids.contains(v)) {
            return Ok(l);
        }
        l.release().await?;
        ids = now;
    }
    Err(conflict("the storage layout kept changing; try again"))
}

/// Raw volumes whose folders overlap this one's on the same storage
/// (including this one): saving a file must lock them all, since the same
/// physical file can be reached through each.
async fn overlapping_raw_volumes(st: &AppState, m: &Mount) -> ApiResult<Vec<i64>> {
    // (volume id, connection id, base path, stored storage id, stored folder)
    type Row = (i64, i64, String, Option<String>, Option<String>);
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT v.id, v.connection_id, v.base_path, c.storage_id, c.storage_folder
         FROM volumes v JOIN connections c ON c.id = v.connection_id WHERE v.mode = 'raw'",
    )
    .fetch_all(&st.db)
    .await?;
    let here = mounts::storage_root(st, m.connection_id, &m.base_path).await?;
    let mut out = vec![m.volume_id];
    let mut locs = mounts::Locations::default();
    for (id, cid, base, sid, sfolder) in rows {
        if id == m.volume_id {
            continue;
        }
        let Some(other) = locs.of(st, cid, &base, (sid, sfolder)).await? else {
            continue; // location unknown: can't reach storage, can't write
        };
        if mounts::overlaps((&here.0, &here.1), (&other.0, &other.1)) {
            out.push(id);
        }
    }
    Ok(out)
}

/// Current state of a file for a precondition check (None if missing).
async fn current_file(st: &AppState, m: &Mount, rel: &str) -> ApiResult<Option<Node>> {
    if m.is_hashed() {
        return get_node(st, m.volume_id, rel).await;
    }
    match stat(st, m, rel).await {
        Ok(n) => Ok(Some(n)),
        Err(ApiError::NotFound) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Point a hashed file at a list of stored chunks. Returns true if it is new.
pub async fn put_hashed_file(
    st: &AppState,
    m: &Mount,
    rel: &str,
    chunks: &[Hash],
    sizes: &[i64],
    mtime: Option<DateTime<Utc>>,
) -> ApiResult<bool> {
    ensure_writable(st, m)?;
    Ok(
        put_hashed_file_if(st, m, rel, chunks, sizes, mtime, &Expect::Any)
            .await?
            .0,
    )
}

/// `put_hashed_file` with a precondition, checked under the volume lock in
/// the same critical section as the write. Returns whether the file is new
/// and the version that was written (taken under the lock, so it is ours).
pub async fn put_hashed_file_if(
    st: &AppState,
    m: &Mount,
    rel: &str,
    chunks: &[Hash],
    sizes: &[i64],
    mtime: Option<DateTime<Utc>>,
    expect: &Expect,
) -> ApiResult<(bool, Node)> {
    ensure_writable(st, m)?;
    let (parent, name) = paths::split(rel).ok_or_else(|| bad("cannot write to a mount point"))?;
    // Saving over an existing file needs no parent folders: if the file is
    // still there, so is its folder. Only plain writes and creations make
    // missing parents, and never for a write that is going to be refused.
    if !parent.is_empty() && !matches!(expect, Expect::Etag { .. }) {
        mkdir_in(st, m, parent).await?;
    }
    let lock = lock_volume(st, m.volume_id).await?;
    if *expect != Expect::Any {
        check_expect(expect, get_node(st, m.volume_id, rel).await?.as_ref())?;
    }
    let content = manifest_hash(chunks);
    let size: i64 = sizes.iter().sum();
    let mtime = mtime.unwrap_or_else(Utc::now);
    let etag = content.hex()[..32].to_string();
    let inserted: Option<bool> = sqlx::query_scalar(
        "INSERT INTO nodes (volume_id, parent, name, is_dir, size, mtime, etag, content_hash, chunks, chunk_sizes)
         VALUES ($1, $2, $3, FALSE, $4, $5, $6, $7, $8, $9)
         ON CONFLICT (volume_id, parent, name) DO UPDATE
           SET size = EXCLUDED.size, mtime = EXCLUDED.mtime, etag = EXCLUDED.etag,
               content_hash = EXCLUDED.content_hash, chunks = EXCLUDED.chunks, chunk_sizes = EXCLUDED.chunk_sizes
           WHERE NOT nodes.is_dir
         RETURNING (xmax = 0)",
    )
    .bind(m.volume_id)
    .bind(parent)
    .bind(name)
    .bind(size)
    .bind(mtime)
    .bind(&etag)
    .bind(content.0.to_vec())
    .bind(chunks.iter().map(|c| c.0.to_vec()).collect::<Vec<_>>())
    .bind(sizes)
    .fetch_optional(&st.db)
    .await?;
    let inserted = inserted.ok_or_else(|| conflict("a folder with that name already exists"))?;
    journal::record_volume(
        st,
        m.volume_id,
        vec![if inserted {
            Change::create(rel, false)
        } else {
            Change::modify(rel)
        }],
    )
    .await?;
    lock.release().await?;
    let node = Node {
        name: name.to_string(),
        is_dir: false,
        size,
        mtime,
        etag: Some(etag),
        chunks: Some(chunks.iter().map(|c| c.0.to_vec()).collect()),
        chunk_sizes: Some(sizes.to_vec()),
    };
    Ok((inserted, node))
}

/// Write a byte stream as a file at `rel` of mount `m`. Returns the BLAKE3 of
/// what was read from the stream.
pub async fn write_stream(
    st: &AppState,
    m: &Mount,
    rel: &str,
    input: ByteStream,
) -> ApiResult<Hash> {
    ensure_writable(st, m)?;
    Ok(write_stream_if(st, m, rel, input, &Expect::Any).await?.0)
}

/// `write_stream` with a precondition. Hashed: checked under the volume lock
/// when the node is written. Raw: the volume lock is taken and the
/// precondition checked (including the base content) before any byte is
/// written, and held until the atomic close, so a refused save writes
/// nothing. Returns the stream's BLAKE3 and
/// the version written, taken under the lock (so it is never someone
/// else's later version).
pub async fn write_stream_if(
    st: &AppState,
    m: &Mount,
    rel: &str,
    mut input: ByteStream,
    expect: &Expect,
) -> ApiResult<(Hash, Node)> {
    ensure_writable(st, m)?;
    // Refuse early (before writing anything, or creating parent folders)
    // when the precondition already fails; re-checked under the lock below.
    // Hashed: refuse early (a cheap DB lookup) before storing any blobs; the
    // authoritative check runs under the lock in `put_hashed_file_if`. Raw
    // conditional writes are checked under the lock before any byte is
    // written (below), so they need no pre-check.
    if m.is_hashed() && *expect != Expect::Any {
        check_expect(expect, get_node(st, m.volume_id, rel).await?.as_ref())?;
    }
    let mut whole = blake3::Hasher::new();
    if m.is_hashed() {
        let store = hashed_store(st, m).await?;
        let mut chunker = StreamChunker::new();
        let mut hashes = Vec::new();
        let mut sizes = Vec::new();
        async fn put(
            st: &AppState,
            m: &Mount,
            store: &HashedStore,
            c: Vec<u8>,
            hs: &mut Vec<Hash>,
            ss: &mut Vec<i64>,
        ) -> ApiResult<()> {
            let h = Hash::of(&c);
            let len = c.len() as i64;
            store_blob(st, store, m.volume_id, &h, Bytes::from(c)).await?;
            hs.push(h);
            ss.push(len);
            Ok(())
        }
        while let Some(b) = input.next().await {
            let b = b.map_err(|e| ApiError::Internal(e.into()))?;
            whole.update(&b);
            for c in chunker.push(&b) {
                put(st, m, &store, c, &mut hashes, &mut sizes).await?;
            }
        }
        for c in chunker.finish() {
            put(st, m, &store, c, &mut hashes, &mut sizes).await?;
        }
        let (_, node) = put_hashed_file_if(st, m, rel, &hashes, &sizes, None, expect).await?;
        Ok((Hash(*whole.finalize().as_bytes()), node))
    } else {
        let op = op(st, m).await?;
        let path = backend_path(m, rel, false);
        // Conditional writes (editor saves, at most 5 MB) are checked under
        // the volume lock *before* any byte is written and keep the lock
        // until the atomic close: a refused save writes nothing at all (no
        // temp files on backends that can't abort, no parent folders).
        // Plain writes (uploads, copies) lock only around the final close.
        let early = if *expect != Expect::Any {
            // Metadata checks run before the lock; the (up to 5 MB) content
            // comparison against the base runs once, under the lock.
            let verified = verify_raw_base(st, m, rel, expect).await?;
            let l = lock_overlapping_raw(st, m).await?;
            recheck_raw_base(st, m, rel, expect, &verified).await?;
            Some(l)
        } else {
            None
        };
        let mut w = op.writer_with(&path).chunk(8 << 20).await?;
        while let Some(b) = input.next().await {
            let b = match b {
                Ok(b) => b,
                Err(e) => {
                    let _ = w.abort().await;
                    return Err(ApiError::Internal(e.into()));
                }
            };
            whole.update(&b);
            w.write(b).await?;
        }
        // The atomic close makes the file visible: do it under the volume lock
        // so the journal entry lands in the same order as the change.
        let lock = match early {
            Some(l) => l,
            None => match lock_volume(st, m.volume_id).await {
                Ok(l) => l,
                Err(e) => {
                    let _ = w.abort().await;
                    return Err(e);
                }
            },
        };
        w.close().await?;
        let node = stat(st, m, rel).await?;
        let created = cache_node(st, m, rel, &node).await?;
        journal::record_volume(
            st,
            m.volume_id,
            vec![if created {
                Change::create(rel, false)
            } else {
                Change::modify(rel)
            }],
        )
        .await?;
        lock.release().await?;
        Ok((Hash(*whole.finalize().as_bytes()), node))
    }
}

// ---------------------------------------------------------------- delete

pub async fn delete(st: &AppState, user_id: i64, path: &str) -> ApiResult<()> {
    if path.is_empty() {
        return Err(bad("cannot delete the root folder"));
    }
    let all = mounts::user_mounts(st, user_id).await?;
    if all.iter().any(|m| m.path == path) {
        return Err(bad("this is a mount point; remove the mount instead"));
    }
    if all
        .iter()
        .any(|m| !m.path.is_empty() && paths::strip_prefix(&m.path, path).is_some())
    {
        return Err(conflict(
            "this folder contains mount points; remove them first",
        ));
    }
    let (m, rel) = mounts::resolve(st, user_id, path).await?;
    delete_in(st, &m, &rel).await
}

pub async fn delete_in(st: &AppState, m: &Mount, rel: &str) -> ApiResult<()> {
    ensure_writable(st, m)?;
    let lock = lock_volume(st, m.volume_id).await?;
    let node = stat(st, m, rel).await?;
    delete_locked(st, m, rel, &node).await?;
    lock.release().await
}

/// Delete a file only if it still has the size and etag of `expected`
/// (checked under the volume lock). Returns false if it changed.
pub async fn delete_if_unchanged(
    st: &AppState,
    m: &Mount,
    rel: &str,
    expected: &Node,
) -> ApiResult<bool> {
    ensure_writable(st, m)?;
    let lock = lock_volume(st, m.volume_id).await?;
    let now = stat(st, m, rel).await?;
    if (now.size, &now.etag) != (expected.size, &expected.etag) {
        return Ok(false);
    }
    delete_locked(st, m, rel, &now).await?;
    lock.release().await?;
    Ok(true)
}

async fn delete_locked(st: &AppState, m: &Mount, rel: &str, node: &Node) -> ApiResult<()> {
    if !m.is_hashed() {
        let op = op(st, m).await?;
        if node.is_dir {
            op.delete_with(&backend_path(m, rel, true))
                .recursive(true)
                .await?;
        } else {
            op.delete(&backend_path(m, rel, false)).await?;
        }
    }
    // Hashed blobs are reclaimed later by GC.
    delete_subtree_rows(st, m.volume_id, rel).await?;
    journal::record_volume(st, m.volume_id, vec![Change::delete(rel, node.is_dir)]).await
}

// ---------------------------------------------------------------- move / copy

/// Move (or rename) `from` to `to` in the user's tree.
///
/// * a mount point: the mount itself moves, no data is touched;
/// * inside one volume: a DB update (hashed) or a backend rename (raw);
/// * across volumes: each file is copied, verified, then deleted; folders are
///   removed only once empty, so nothing is lost if an entry cannot be copied.
pub async fn move_path(st: &AppState, user_id: i64, from: &str, to: &str) -> ApiResult<()> {
    check_move_target(from, to)?;
    let all = mounts::user_mounts(st, user_id).await?;
    if all.iter().any(|m| m.path == to) {
        return Err(conflict("something is mounted at the destination"));
    }
    let (dm, drel) = mounts::resolve(st, user_id, to).await?;
    if stat(st, &dm, &drel).await.is_ok() {
        return Err(conflict("destination already exists"));
    }
    let is_mount_point = all.iter().any(|m| m.path == from);
    let source = if is_mount_point {
        None
    } else {
        let (sm, srel) = mounts::resolve(st, user_id, from).await?;
        ensure_writable(st, &sm)?;
        ensure_writable(st, &dm)?;
        let node = stat(st, &sm, &srel).await?;
        check_not_into_itself(&sm, &srel, &dm, &drel)?;
        Some((sm, srel, node))
    };
    // Only now, with every check passed, create the destination's parents.
    let (to_parent, _) = paths::split(to).unwrap();
    mkdir_p(st, user_id, to_parent).await?;
    let (dm, drel) = mounts::resolve(st, user_id, to).await?;
    let mut left = 0;
    if let Some((sm, srel, node)) = source {
        if sm.volume_id == dm.volume_id {
            move_within(st, &sm, &srel, &drel, node.is_dir).await?;
        } else {
            left = move_tree(st, &sm, &srel, &dm, &drel, node.is_dir).await?;
        }
    }
    // Mount points at or below `from` move along.
    shift_mounts(st, user_id, from, to).await?;
    if is_mount_point {
        journal::record_user(st, user_id, vec![Change::moved(from, to, true)]).await?;
    }
    if left > 0 {
        return Err(conflict(format!(
            "{left} item(s) could not be moved and are still in the source folder"
        )));
    }
    Ok(())
}

/// Entries of a folder (rescanning raw folders first).
async fn children(st: &AppState, m: &Mount, rel: &str) -> ApiResult<Vec<Node>> {
    if !m.is_hashed() {
        scan_dir(st, m, rel).await?;
    }
    Ok(sqlx::query_as(
        "SELECT name, is_dir, size, mtime, etag, chunks, chunk_sizes FROM nodes
         WHERE volume_id = $1 AND parent = $2",
    )
    .bind(m.volume_id)
    .bind(rel)
    .fetch_all(&st.db)
    .await?)
}

/// Is a folder empty on its storage (backend for raw, tree for hashed)?
async fn is_empty_dir(st: &AppState, m: &Mount, rel: &str) -> ApiResult<bool> {
    if m.is_hashed() {
        return Ok(children(st, m, rel).await?.is_empty());
    }
    let dir = backend_path(m, rel, true);
    let entries = op(st, m).await?.list(&dir).await?;
    let me = dir.trim_start_matches('/');
    Ok(entries
        .iter()
        .all(|e| e.path().trim_start_matches('/') == me))
}

/// Remove a folder if it is (still) empty, checked under the volume lock.
/// On object stores the folder may already be gone (it only existed through
/// its contents). Returns false if something appeared in it meanwhile.
async fn remove_empty_dir(st: &AppState, m: &Mount, rel: &str) -> ApiResult<bool> {
    let lock = lock_volume(st, m.volume_id).await?;
    let has_rows: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM nodes WHERE volume_id = $1 AND parent = $2)",
    )
    .bind(m.volume_id)
    .bind(rel)
    .fetch_one(&st.db)
    .await?;
    if has_rows || (!m.is_hashed() && !is_empty_dir(st, m, rel).await?) {
        return Ok(false);
    }
    if !m.is_hashed() {
        match op(st, m).await?.delete(&backend_path(m, rel, true)).await {
            Ok(()) => {}
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    let (parent, name) = paths::split(rel).ok_or_else(|| bad("cannot delete the root"))?;
    sqlx::query("DELETE FROM nodes WHERE volume_id = $1 AND parent = $2 AND name = $3 AND is_dir")
        .bind(m.volume_id)
        .bind(parent)
        .bind(name)
        .execute(&st.db)
        .await?;
    sqlx::query("DELETE FROM raw_scans WHERE volume_id = $1 AND dir = $2")
        .bind(m.volume_id)
        .bind(rel)
        .execute(&st.db)
        .await?;
    journal::record_volume(st, m.volume_id, vec![Change::delete(rel, true)]).await?;
    lock.release().await?;
    Ok(true)
}

/// Move across volumes. Returns how many entries had to stay behind.
async fn move_tree(
    st: &AppState,
    sm: &Mount,
    srel: &str,
    dm: &Mount,
    drel: &str,
    is_dir: bool,
) -> ApiResult<usize> {
    if !is_dir {
        let before = stat(st, sm, srel).await?;
        copy_tree(st, sm, srel, dm, drel, false).await?; // verified copy
        // Delete only the version we copied; an edit made meanwhile is kept.
        return Ok(if delete_if_unchanged(st, sm, srel, &before).await? {
            0
        } else {
            1
        });
    }
    mkdir_in(st, dm, drel).await?;
    let mut left = 0;
    for c in children(st, sm, srel).await? {
        let s = paths::join(srel, &c.name);
        let d = paths::join(drel, &c.name);
        left += Box::pin(move_tree(st, sm, &s, dm, &d, c.is_dir)).await?;
    }
    if left > 0 || !remove_empty_dir(st, sm, srel).await? {
        left += 1; // entries we could not list or move (e.g. invalid names)
    }
    Ok(left)
}

fn check_move_target(from: &str, to: &str) -> ApiResult<()> {
    if from.is_empty() || to.is_empty() {
        return Err(bad("cannot move the root folder"));
    }
    if from == to {
        return Err(bad("source and destination are the same"));
    }
    if paths::strip_prefix(to, from).is_some() {
        return Err(bad("cannot move a folder into itself"));
    }
    Ok(())
}

/// The user-path check is not enough: two mounts may show the same volume, or
/// overlapping folders of one connection (raw). Compare storage locations too.
fn check_not_into_itself(sm: &Mount, srel: &str, dm: &Mount, drel: &str) -> ApiResult<()> {
    let same_volume = sm.volume_id == dm.volume_id && paths::strip_prefix(drel, srel).is_some();
    let overlapping_raw = !sm.is_hashed()
        && !dm.is_hashed()
        && sm.connection_id == dm.connection_id
        && paths::strip_prefix(
            &paths::join(&dm.base_path, drel),
            &paths::join(&sm.base_path, srel),
        )
        .is_some();
    if same_volume || overlapping_raw {
        return Err(bad("cannot move or copy a folder into itself"));
    }
    Ok(())
}

async fn shift_mounts(st: &AppState, user_id: i64, from: &str, to: &str) -> ApiResult<()> {
    let all = mounts::user_mounts(st, user_id).await?;
    let mut changed = false;
    for m in all.iter().filter(|m| !m.path.is_empty()) {
        if let Some(rest) = paths::strip_prefix(&m.path, from) {
            let np = paths::join(to, rest);
            let (pp, name) = paths::split(&np).unwrap();
            sqlx::query("UPDATE mounts SET path = $2, parent_path = $3, name = $4 WHERE id = $1")
                .bind(m.id)
                .bind(&np)
                .bind(pp)
                .bind(name)
                .execute(&st.db)
                .await?;
            changed = true;
        }
    }
    if changed {
        mounts::invalidate(st, user_id);
    }
    Ok(())
}

async fn move_within(
    st: &AppState,
    m: &Mount,
    from: &str,
    to: &str,
    is_dir: bool,
) -> ApiResult<()> {
    let local_dir = if !m.is_hashed() && m.connection_kind == "local" {
        st.connection_config(m.connection_id)
            .await?
            .local_dir(&st.cfg.local_base)?
    } else {
        None
    };
    let op = if m.is_hashed() {
        None
    } else {
        Some(op(st, m).await?)
    };
    let native_rename = local_dir.is_some()
        || (!is_dir && op.as_ref().is_some_and(|o| o.info().capability().rename));
    if !m.is_hashed() && !native_rename {
        // No native rename (e.g. S3): copy then delete, still in one volume.
        let left = move_tree(st, m, from, m, to, is_dir).await?;
        if left > 0 {
            return Err(conflict(format!(
                "{left} item(s) could not be moved and are still in the source folder"
            )));
        }
        return Ok(());
    }
    // Backend rename, cache update and journal entry under one volume lock.
    let lock = lock_volume(st, m.volume_id).await?;
    // The destination was free when the move started; make sure it still is.
    if stat(st, m, to).await.is_ok() {
        return Err(conflict("destination already exists"));
    }
    if !m.is_hashed() {
        if let Some(root) = local_dir {
            // Local folders: a real rename(2), also for directories.
            let a = root.join(paths::join(&m.base_path, from));
            let b = root.join(paths::join(&m.base_path, to));
            if let Some(p) = b.parent() {
                tokio::fs::create_dir_all(p)
                    .await
                    .map_err(|e| ApiError::Internal(e.into()))?;
            }
            tokio::fs::rename(&a, &b)
                .await
                .map_err(|e| ApiError::Internal(e.into()))?;
        } else if let Some(op) = &op {
            op.rename(&backend_path(m, from, false), &backend_path(m, to, false))
                .await?;
        }
    }
    let (op_, on) = paths::split(from).unwrap();
    let (np, nn) = paths::split(to).unwrap();
    let mut tx = st.db.begin().await?;
    sqlx::query("UPDATE nodes SET parent = $4, name = $5 WHERE volume_id = $1 AND parent = $2 AND name = $3")
        .bind(m.volume_id)
        .bind(op_)
        .bind(on)
        .bind(np)
        .bind(nn)
        .execute(&mut *tx)
        .await?;
    if is_dir {
        let like = format!("{}/%", like_escape(from));
        sqlx::query(
            "UPDATE nodes SET parent = $3 || substr(parent, length($2) + 1)
             WHERE volume_id = $1 AND (parent = $2 OR parent LIKE $4)",
        )
        .bind(m.volume_id)
        .bind(from)
        .bind(to)
        .bind(&like)
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM raw_scans WHERE volume_id = $1 AND (dir = $2 OR dir LIKE $3)")
            .bind(m.volume_id)
            .bind(from)
            .bind(&like)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    if !m.is_hashed() {
        // The source may not have been cached; make sure the destination is,
        // and have both parent folders re-listed next time.
        let n = stat(st, m, to).await?;
        cache_node(st, m, to, &n).await?;
        let parents = [op_.to_string(), np.to_string()];
        sqlx::query("DELETE FROM raw_scans WHERE volume_id = $1 AND dir = ANY($2)")
            .bind(m.volume_id)
            .bind(&parents[..])
            .execute(&st.db)
            .await?;
    }
    journal::record_volume(st, m.volume_id, vec![Change::moved(from, to, is_dir)]).await?;
    lock.release().await
}

/// Copy a file or folder tree between (or within) volumes, verifying every file.
pub async fn copy_tree(
    st: &AppState,
    sm: &Mount,
    srel: &str,
    dm: &Mount,
    drel: &str,
    is_dir: bool,
) -> ApiResult<()> {
    ensure_writable(st, dm)?;
    if !is_dir {
        let node = stat(st, sm, srel).await?;
        let input = read_stream(st, sm, srel, &node, 0, node.size as u64).await?;
        let src_hash = write_stream(st, dm, drel, input).await?;
        let dst_hash = content_digest(st, dm, drel).await?;
        if src_hash != dst_hash {
            let _ = delete_in(st, dm, drel).await;
            return Err(ApiError::Internal(anyhow::anyhow!(
                "verification failed copying {srel}"
            )));
        }
        return Ok(());
    }
    mkdir_in(st, dm, drel).await?;
    let children: Vec<Node> = if sm.is_hashed() {
        sqlx::query_as(
            "SELECT name, is_dir, size, mtime, etag, chunks, chunk_sizes FROM nodes WHERE volume_id = $1 AND parent = $2",
        )
        .bind(sm.volume_id)
        .bind(srel)
        .fetch_all(&st.db)
        .await?
    } else {
        scan_dir(st, sm, srel).await?;
        sqlx::query_as(
            "SELECT name, is_dir, size, mtime, etag, chunks, chunk_sizes FROM nodes WHERE volume_id = $1 AND parent = $2",
        )
        .bind(sm.volume_id)
        .bind(srel)
        .fetch_all(&st.db)
        .await?
    };
    for c in children {
        let s = paths::join(srel, &c.name);
        let d = paths::join(drel, &c.name);
        Box::pin(copy_tree(st, sm, &s, dm, &d, c.is_dir)).await?;
    }
    Ok(())
}

/// Copy `from` to `to` in the user's tree.
pub async fn copy_path(st: &AppState, user_id: i64, from: &str, to: &str) -> ApiResult<()> {
    check_move_target(from, to)?;
    let (sm, srel) = mounts::resolve(st, user_id, from).await?;
    let (dm, drel) = mounts::resolve(st, user_id, to).await?;
    if drel.is_empty() || stat(st, &dm, &drel).await.is_ok() {
        return Err(conflict("destination already exists"));
    }
    check_not_into_itself(&sm, &srel, &dm, &drel)?;
    let node = stat(st, &sm, &srel).await?;
    let (to_parent, _) = paths::split(to).unwrap();
    mkdir_p(st, user_id, to_parent).await?;
    if sm.is_hashed()
        && sm.volume_id == dm.volume_id
        && !node.is_dir
        && touch_blobs(st, sm.volume_id, node.chunks.as_deref().unwrap_or_default())
            .await?
            .is_empty()
    {
        // Same store: share the blobs (refreshed above, so GC keeps them).
        let chunks: Vec<Hash> = node
            .chunks
            .unwrap_or_default()
            .iter()
            .filter_map(|c| Hash::from_slice(c))
            .collect();
        put_hashed_file(
            st,
            &dm,
            &drel,
            &chunks,
            &node.chunk_sizes.unwrap_or_default(),
            None,
        )
        .await?;
        return Ok(());
    }
    copy_tree(st, &sm, &srel, &dm, &drel, node.is_dir).await
}

// ---------------------------------------------------------------- search

#[derive(Serialize, sqlx::FromRow)]
pub struct SearchHit {
    pub path: String,
    pub name: String,
    pub is_dir: bool,
    pub size: i64,
    pub mtime: DateTime<Utc>,
}

pub async fn search(st: &AppState, user_id: i64, q: &str, limit: i64) -> ApiResult<Vec<SearchHit>> {
    let pattern = format!("%{}%", like_escape(q));
    let hits: Vec<SearchHit> = sqlx::query_as(
        "SELECT CASE WHEN m.path = '' THEN '' ELSE m.path || '/' END
                || CASE WHEN n.parent = '' THEN '' ELSE n.parent || '/' END || n.name AS path,
                n.name, n.is_dir, n.size, n.mtime
         FROM nodes n JOIN mounts m ON m.volume_id = n.volume_id
         WHERE m.owner_id = $1 AND n.name ILIKE $2
         ORDER BY similarity(n.name, $3) DESC, n.name
         LIMIT $4",
    )
    .bind(user_id)
    .bind(&pattern)
    .bind(q)
    .bind(limit)
    .fetch_all(&st.db)
    .await?;
    Ok(hits)
}
