//! Trash: deleting a file or folder moves it here, where it waits until it is
//! restored, purged by hand, or purged after `STRATA_TRASH_DAYS`.
//!
//! * HASHED volumes: the item's `nodes` rows move to `trash_nodes` in one
//!   transaction, so its blobs stay referenced (GC counts `trash_nodes`) and
//!   nothing on the backend changes.
//! * RAW volumes on local connections: the item is renamed (rename(2), one
//!   filesystem) to `<connection root>/.strata-trash/<id>/<name>`. The row is
//!   written first with `ready = false`; a crash before or after the rename is
//!   settled by `reconcile`, so the item is always either in place or in the
//!   trash, never lost.
//! * Other raw backends (S3, FTP) have no cheap, safe rename: deletes there
//!   stay permanent, and the listing tells the client (`mount.trash`).
//!
//! Every step that changes an item runs under the volume lock, so trashing,
//! restoring, purging and reconciling one volume never interleave.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use axum::Json;
use axum::extract::{Path as UrlPath, Query, State};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use strata_storage::backend::TRASH_DIR;
use strata_storage::paths;

use crate::auth::AuthUser;
use crate::error::{ApiError, ApiResult, bad, conflict, io_error};
use crate::journal::{self, Change};
use crate::mounts::{self, Mount};
use crate::state::AppState;
use crate::vfs;

/// Can deletes in this mount go to the trash?
pub fn supports_trash(m: &Mount) -> bool {
    m.is_hashed() || m.connection_kind == "local"
}

/// Is the trash turned on (`STRATA_TRASH_DAYS` > 0)?
pub fn enabled(st: &AppState) -> bool {
    st.cfg.trash_days > 0
}

#[derive(Clone, Debug, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct Item {
    pub id: i64,
    pub name: String,
    pub original_path: String,
    pub is_dir: bool,
    pub size: i64,
    pub deleted_at: DateTime<Utc>,
    pub purge_after: DateTime<Utc>,
}

#[derive(Clone, Debug, sqlx::FromRow)]
struct Row {
    id: i64,
    volume_id: i64,
    rel_path: String,
    name: String,
    is_dir: bool,
}

/// The folder of a local connection that the trash lives in.
async fn local_root(st: &AppState, connection_id: i64) -> ApiResult<PathBuf> {
    st.connection_config(connection_id)
        .await?
        .local_dir(&st.cfg.local_base)?
        .ok_or_else(|| ApiError::Internal(anyhow::anyhow!("not a local connection")))
}

fn item_dir(root: &Path, id: i64) -> PathBuf {
    root.join(TRASH_DIR).join(id.to_string())
}

/// Where a purge moves an item before removing it outside the volume lock.
/// Whatever is found under this name is doomed (the sweep finishes it).
fn doomed_dir(root: &Path, id: i64) -> PathBuf {
    let nanos = Utc::now().timestamp_nanos_opt().unwrap_or_default();
    root.join(TRASH_DIR).join(format!("purging-{id}-{nanos}"))
}

/// Move a `.strata-trash/<id>` folder no row knows out of the way, keeping
/// its contents (it may hold files after a database restore from backup).
async fn set_aside(root: &Path, id: i64) -> std::io::Result<()> {
    let secs = Utc::now().timestamp();
    let to = root.join(TRASH_DIR).join(format!("orphan-{id}-{secs}"));
    tokio::fs::rename(item_dir(root, id), to).await
}

/// Make a fresh, empty `.strata-trash/<id>` folder. One left over from a
/// database restore is set aside first, never mixed with or replaced.
async fn make_item_dir(root: &Path, id: i64) -> std::io::Result<PathBuf> {
    let dir = item_dir(root, id);
    tokio::fs::create_dir_all(root.join(TRASH_DIR)).await?;
    match tokio::fs::create_dir(&dir).await {
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            set_aside(root, id).await?;
            tokio::fs::create_dir(&dir).await?;
        }
        r => r?,
    }
    Ok(dir)
}

fn subtree_like(rel: &str) -> String {
    format!("{}/%", vfs::like_escape(rel))
}

/// Remember the share links that point into an item about to be trashed
/// ($1 trash id, $2 volume, $3/$4 parent and name, $5 the item, $6 below it).
const REMEMBER_SHARES: &str = "INSERT INTO trash_shares (trash_id, share_id, node_id)
     SELECT $1, s.id, s.node_id FROM shares s JOIN nodes n ON n.id = s.node_id
     WHERE n.volume_id = $2 AND ((n.parent = $3 AND n.name = $4) OR n.parent = $5 OR n.parent LIKE $6)";

/// Move `rel` of mount `m` (shown to the user as `original_path`) to the
/// owner's trash. The caller has checked that it is not a mount point and
/// holds no mount points. Returns the trash id.
pub async fn trash(
    st: &AppState,
    user_id: i64,
    original_path: &str,
    m: &Mount,
    rel: &str,
) -> ApiResult<i64> {
    if !supports_trash(m) {
        return Err(bad("this storage has no trash"));
    }
    let (parent, name) = paths::split(rel).ok_or_else(|| bad("cannot delete the root folder"))?;
    vfs::ensure_writable(st, m).await?;
    let lock = vfs::lock_volume(st, m.volume_id).await?;
    // Under the lock: a connection being pointed elsewhere holds it.
    let root = if m.is_hashed() {
        None
    } else {
        Some(local_root(st, m.connection_id).await?)
    };
    let node = vfs::stat(st, m, rel).await?;
    let like = subtree_like(rel);
    let id = match &root {
        None => {
            let mut tx = st.db.begin().await?;
            let id: i64 = sqlx::query_scalar(
                "INSERT INTO trash (owner_id, volume_id, original_path, rel_path, name, is_dir, ready, purge_after)
                 VALUES ($1, $2, $3, $4, $5, $6, TRUE, now() + make_interval(days => $7))
                 RETURNING id",
            )
            .bind(user_id)
            .bind(m.volume_id)
            .bind(original_path)
            .bind(rel)
            .bind(name)
            .bind(node.is_dir)
            .bind(st.cfg.trash_days as i32)
            .fetch_one(&mut *tx)
            .await?;
            sqlx::query(
                "INSERT INTO trash_nodes (trash_id, node_id, volume_id, parent, name, is_dir, size, mtime, etag, content_hash, chunks, chunk_sizes)
                 SELECT $1, id, volume_id, parent, name, is_dir, size, mtime, etag, content_hash, chunks, chunk_sizes
                 FROM nodes
                 WHERE volume_id = $2 AND ((parent = $3 AND name = $4) OR parent = $5 OR parent LIKE $6)",
            )
            .bind(id)
            .bind(m.volume_id)
            .bind(parent)
            .bind(name)
            .bind(rel)
            .bind(&like)
            .execute(&mut *tx)
            .await?;
            sqlx::query(REMEMBER_SHARES)
                .bind(id)
                .bind(m.volume_id)
                .bind(parent)
                .bind(name)
                .bind(rel)
                .bind(&like)
                .execute(&mut *tx)
                .await?;
            sqlx::query(
                "UPDATE trash SET size = (SELECT coalesce(sum(size), 0)::bigint FROM trash_nodes
                                          WHERE trash_id = $1 AND NOT is_dir)
                 WHERE id = $1",
            )
            .bind(id)
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "DELETE FROM nodes
                 WHERE volume_id = $1 AND ((parent = $2 AND name = $3) OR parent = $4 OR parent LIKE $5)",
            )
            .bind(m.volume_id)
            .bind(parent)
            .bind(name)
            .bind(rel)
            .bind(&like)
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
            id
        }
        Some(root) => {
            // Size from the listing cache: exact for a file, what has been
            // seen so far for a folder.
            let size: i64 = if node.is_dir {
                sqlx::query_scalar(
                    "SELECT coalesce(sum(size), 0)::bigint FROM nodes
                     WHERE volume_id = $1 AND NOT is_dir AND (parent = $2 OR parent LIKE $3)",
                )
                .bind(m.volume_id)
                .bind(rel)
                .bind(&like)
                .fetch_one(&st.db)
                .await?
            } else {
                node.size
            };
            // Recorded (committed) before the rename: a crash after the
            // rename still finds the item through this row.
            let node_id = vfs::node_id(st, m.volume_id, rel).await?;
            let id: i64 = sqlx::query_scalar(
                "INSERT INTO trash (owner_id, volume_id, original_path, rel_path, name, is_dir, size, node_id, ready, purge_after)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $9, FALSE, now() + make_interval(days => $8))
                 RETURNING id",
            )
            .bind(user_id)
            .bind(m.volume_id)
            .bind(original_path)
            .bind(rel)
            .bind(name)
            .bind(node.is_dir)
            .bind(size)
            .bind(st.cfg.trash_days as i32)
            .bind(node_id)
            .fetch_one(&st.db)
            .await?;
            // Before the rename too: once the item is moved, everything below
            // is best effort (reconcile finishes the move after a crash).
            if let Err(e) = sqlx::query(REMEMBER_SHARES)
                .bind(id)
                .bind(m.volume_id)
                .bind(parent)
                .bind(name)
                .bind(rel)
                .bind(&like)
                .execute(&st.db)
                .await
            {
                let _ = sqlx::query("DELETE FROM trash WHERE id = $1")
                    .bind(id)
                    .execute(&st.db)
                    .await;
                return Err(e.into());
            }
            let src = root.join(paths::join(&m.base_path, rel));
            let dir = item_dir(root, id);
            let moved = async {
                make_item_dir(root, id).await?;
                tokio::fs::rename(&src, dir.join(name)).await
            }
            .await;
            if let Err(e) = moved {
                // Nothing moved: forget the row (reconcile does it if this fails).
                let _ = tokio::fs::remove_dir(&dir).await;
                let _ = sqlx::query("DELETE FROM trash WHERE id = $1")
                    .bind(id)
                    .execute(&st.db)
                    .await;
                return Err(
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::CrossesDevices
                            | std::io::ErrorKind::ResourceBusy
                            | std::io::ErrorKind::PermissionDenied
                            | std::io::ErrorKind::ReadOnlyFilesystem
                    ) {
                        // A disk mounted inside the connection (or on the item
                        // itself), or a trash folder that can't be written: no
                        // rename to the trash is possible. Say so; the client
                        // may then delete permanently.
                        conflict(
                            "this item can't be moved to the trash (it is on another disk than its connection, or the trash folder can't be written); delete it permanently instead",
                        )
                    } else {
                        io_error(e)
                    },
                );
            }
            // The item is in the trash now: report success even if the
            // bookkeeping below fails (reconcile completes it).
            if let Err(e) = finish_raw_trash(st, root, id, m.volume_id, rel, node.is_dir).await {
                tracing::warn!("trash {id}: finishing the move failed, reconcile will: {e}");
            }
            lock.release().await?;
            return Ok(id);
        }
    };
    // Done already: a failed feed entry is logged, not an error to retry.
    if let Err(e) =
        journal::record_volume(st, m.volume_id, vec![Change::delete(rel, node.is_dir)]).await
    {
        tracing::warn!("trash {id}: journaling the delete failed: {e}");
    }
    lock.release().await?;
    Ok(id)
}

/// After a raw item was renamed into the trash: mark the row ready, drop the
/// item's listing cache and journal the delete. Caller holds the volume lock.
async fn finish_raw_trash(
    st: &AppState,
    root: &Path,
    id: i64,
    volume_id: i64,
    rel: &str,
    is_dir: bool,
) -> ApiResult<()> {
    // Ready last: if a step fails, reconcile repeats them all.
    vfs::delete_subtree_rows(st, volume_id, rel).await?;
    journal::record_volume(st, volume_id, vec![Change::delete(rel, is_dir)]).await?;
    let marked = sqlx::query("UPDATE trash SET ready = TRUE WHERE id = $1")
        .bind(id)
        .execute(&st.db)
        .await?;
    if marked.rows_affected() == 0 {
        // The row went meanwhile (its owner or connection was deleted, which
        // removes their trash): the moved item goes too, by the sweep.
        if let Err(e) = tokio::fs::rename(item_dir(root, id), doomed_dir(root, id)).await {
            tracing::warn!("trash {id}: its record is gone, and dooming the files failed: {e}");
        }
    }
    Ok(())
}

async fn owned_row(st: &AppState, user_id: i64, id: i64) -> ApiResult<Row> {
    sqlx::query_as(
        "SELECT id, volume_id, rel_path, name, is_dir FROM trash
         WHERE id = $1 AND owner_id = $2 AND ready",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(&st.db)
    .await?
    .ok_or(ApiError::NotFound)
}

/// Re-read a row under the volume lock (a purge or restore may have won).
async fn row_again(st: &AppState, id: i64) -> ApiResult<Row> {
    sqlx::query_as(
        "SELECT id, volume_id, rel_path, name, is_dir FROM trash WHERE id = $1 AND ready",
    )
    .bind(id)
    .fetch_optional(&st.db)
    .await?
    .ok_or(ApiError::NotFound)
}

/// "name (restored).ext", then "name (restored 2).ext", ... Folders keep
/// their whole name: "v1.2 (restored)".
pub fn restored_name(name: &str, is_dir: bool, n: u32) -> String {
    let tag = if n <= 1 {
        "restored".to_string()
    } else {
        format!("restored {n}")
    };
    // A leading dot is part of the name (".env"), not an extension.
    match name.rfind('.') {
        Some(i) if i > 0 && !is_dir => format!("{} ({tag}){}", &name[..i], &name[i..]),
        _ => format!("{name} ({tag})"),
    }
}

/// Refuse a restore whose parent folder another mount now covers: the item
/// would land out of sight (also another mount of the same volume, as the
/// returned path would resolve through that one).
fn refuse_covered(all: &[Mount], m: &Mount, parent: &str) -> ApiResult<()> {
    let user_parent = paths::join(&m.path, parent);
    if all.iter().any(|x| {
        x.path.len() > m.path.len() && paths::strip_prefix(&user_parent, &x.path).is_some()
    }) {
        return Err(conflict(
            "something is mounted where this was; remove that mount to restore it",
        ));
    }
    Ok(())
}

/// The original name if it is free in `parent`, else the first free
/// "(restored)" variant. Caller holds the volume lock.
async fn free_name(
    st: &AppState,
    user_id: i64,
    m: &Mount,
    parent: &str,
    name: &str,
    is_dir: bool,
) -> ApiResult<String> {
    // Something the user mounted since then may cover the place: the parent
    // (refuse, it would be hidden) or the name itself (taken).
    let all = mounts::user_mounts(st, user_id).await?;
    let user_parent = paths::join(&m.path, parent);
    refuse_covered(&all, m, parent)?;
    for n in 0..1000 {
        let candidate = if n == 0 {
            name.to_string()
        } else {
            restored_name(name, is_dir, n)
        };
        if candidate.len() > 255 {
            return Err(conflict(
                "the name is too long to add \"(restored)\"; rename the item that has its name, then try again",
            ));
        }
        if all
            .iter()
            .any(|x| x.path == paths::join(&user_parent, &candidate))
        {
            continue;
        }
        match vfs::stat(st, m, &paths::join(parent, &candidate)).await {
            Err(ApiError::NotFound) => return Ok(candidate),
            Err(e) => return Err(e),
            Ok(_) => {}
        }
    }
    Err(conflict("too many items with that name"))
}

/// Move `src` to `dst` only if nothing is there, so an entry created outside
/// Strata meanwhile is never replaced:
///
/// * a file is hard-linked (which fails if `dst` exists), then unlinked from
///   `src`; if that unlink fails the link is undone, leaving one copy;
/// * a folder claims `dst` with mkdir (which fails if it exists), then is
///   renamed over that empty folder of ours (rename never replaces a
///   non-empty folder, so files put there meanwhile make it fail instead).
///
/// Filesystems without hard links fall back to a checked rename.
async fn rename_no_replace(src: &Path, dst: &Path) -> std::io::Result<()> {
    // The entry's own kind: a symlink to a folder moves as a link (a file).
    let is_dir = tokio::fs::symlink_metadata(src).await?.is_dir();
    if is_dir {
        tokio::fs::create_dir(dst).await?;
        if let Err(e) = tokio::fs::rename(src, dst).await {
            let _ = tokio::fs::remove_dir(dst).await;
            return Err(e);
        }
        return Ok(());
    }
    match tokio::fs::hard_link(src, dst).await {
        Ok(()) => {
            if let Err(e) = tokio::fs::remove_file(src).await {
                if let Err(undo) = tokio::fs::remove_file(dst).await {
                    tracing::warn!("restore: undoing a link failed: {undo}");
                }
                return Err(e);
            }
            return Ok(());
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return Err(e),
        Err(_) => {} // no hard links here: fall back to rename
    }
    if tokio::fs::symlink_metadata(dst).await.is_ok() {
        return Err(std::io::ErrorKind::AlreadyExists.into());
    }
    tokio::fs::rename(src, dst).await
}

/// Put a trashed item back where it was (under a "(restored)" name if that
/// name is taken), recreating missing parent folders. Returns its path in
/// the user's tree.
pub async fn restore(st: &AppState, user_id: i64, id: i64) -> ApiResult<String> {
    let row = owned_row(st, user_id, id).await?;
    // From here on the stored id names files on disk, not the request's.
    let id = row.id;
    let all = mounts::user_mounts(st, user_id).await?;
    let m = all
        .iter()
        .filter(|m| m.volume_id == row.volume_id)
        .min_by_key(|m| m.path.len())
        .cloned()
        .ok_or_else(|| conflict("the storage this came from is no longer mounted"))?;
    vfs::ensure_writable(st, &m).await?;
    let (parent, _) = paths::split(&row.rel_path).ok_or_else(|| bad("invalid trash entry"))?;
    let root = if m.is_hashed() {
        None
    } else {
        Some(local_root(st, m.connection_id).await?)
    };
    // Missing parents are created first (mkdir takes the volume lock itself),
    // then checked again under the lock: a delete may remove them in between.
    // (Still there? A racing purge or restore would leave the folders behind.)
    row_again(st, id).await?;
    // Before making any folder (checked again under the lock below).
    refuse_covered(&all, &m, parent)?;
    // A raw item that isn't there (gone, or its disk not mounted) makes no
    // folders: the check under the lock below says which it is.
    let missing = match &root {
        Some(r) => tokio::fs::symlink_metadata(item_dir(r, id).join(&row.name))
            .await
            .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound),
        None => false,
    };
    let mut tries = 0;
    let lock = loop {
        if missing {
            break vfs::lock_volume(st, m.volume_id).await?;
        }
        if !parent.is_empty() {
            vfs::mkdir_in(st, &m, parent).await?;
        }
        let lock = vfs::lock_volume(st, m.volume_id).await?;
        if parent.is_empty() || vfs::stat(st, &m, parent).await.is_ok_and(|n| n.is_dir) {
            break lock;
        }
        lock.release().await?;
        tries += 1;
        if tries == 3 {
            return Err(conflict(
                "the folder it goes back to keeps changing; try again",
            ));
        }
    };
    let row = row_again(st, id).await?;
    let name = free_name(st, user_id, &m, parent, &row.name, row.is_dir).await?;
    let new_rel = paths::join(parent, &name);
    match &root {
        None => {
            let mut tx = st.db.begin().await?;
            sqlx::query(
                "INSERT INTO nodes (id, volume_id, parent, name, is_dir, size, mtime, etag, content_hash, chunks, chunk_sizes)
                 SELECT node_id, volume_id,
                        CASE WHEN parent = $2 AND name = $3 THEN parent ELSE $4 || substr(parent, length($5) + 1) END,
                        CASE WHEN parent = $2 AND name = $3 THEN $6 ELSE name END,
                        is_dir, size, mtime, etag, content_hash, chunks, chunk_sizes
                 FROM trash_nodes WHERE trash_id = $1",
            )
            .bind(id)
            .bind(parent)
            .bind(&row.name)
            .bind(&new_rel)
            .bind(&row.rel_path)
            .bind(&name)
            .execute(&mut *tx)
            .await?;
            if name == row.name {
                // Same ids at the same paths: the share links work again.
                // (A link names a path too: under another name it stays dead.)
                sqlx::query(
                    "UPDATE shares s SET node_id = t.node_id FROM trash_shares t
                     WHERE t.trash_id = $1 AND s.id = t.share_id AND s.node_id IS NULL",
                )
                .bind(id)
                .execute(&mut *tx)
                .await?;
            }
            sqlx::query("DELETE FROM trash WHERE id = $1")
                .bind(id)
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
        }
        Some(root) => {
            let dir = item_dir(root, id);
            let item = dir.join(&row.name);
            match tokio::fs::symlink_metadata(&item).await {
                Ok(_) => {}
                // Gone from disk (removed outside Strata): nothing to
                // restore. Only believed while the trash folder is there.
                Err(e)
                    if e.kind() == std::io::ErrorKind::NotFound
                        && trash_reachable(st, m.connection_id, root).await =>
                {
                    sqlx::query("DELETE FROM trash WHERE id = $1")
                        .bind(id)
                        .execute(&st.db)
                        .await?;
                    let _ = tokio::fs::remove_dir(&dir).await;
                    lock.release().await?;
                    return Err(ApiError::NotFound);
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    return Err(ApiError::Unavailable(
                        "this storage is not available right now; try again later".into(),
                    ));
                }
                Err(e) => return Err(io_error(e)),
            }
            let dst = root.join(paths::join(&m.base_path, &new_rel));
            rename_no_replace(&item, &dst)
                .await
                .map_err(|e| match e.kind() {
                    std::io::ErrorKind::AlreadyExists | std::io::ErrorKind::DirectoryNotEmpty => {
                        conflict("something with that name appeared; try again")
                    }
                    std::io::ErrorKind::CrossesDevices | std::io::ErrorKind::ResourceBusy => {
                        conflict(
                            "another disk is mounted where this was, so it can't be moved back",
                        )
                    }
                    _ => io_error(e),
                })?;
            // The item is back: from here on, a failure must not leave it
            // listed in the trash too.
            if let Err(e) =
                finish_raw_restore(st, &m, id, parent, &name, &new_rel, name == row.name, &dir)
                    .await
            {
                tracing::warn!("restore {id}: bookkeeping after the move failed: {e}");
                let _ = sqlx::query("DELETE FROM trash WHERE id = $1")
                    .bind(id)
                    .execute(&st.db)
                    .await;
                // At least make the next listing of the parent look again.
                let _ = sqlx::query("DELETE FROM raw_scans WHERE volume_id = $1 AND dir = $2")
                    .bind(m.volume_id)
                    .bind(parent)
                    .execute(&st.db)
                    .await;
                let _ = tokio::fs::remove_dir(&dir).await;
            }
        }
    }
    // Done already: a failed feed entry is logged, not an error to retry.
    if let Err(e) =
        journal::record_volume(st, m.volume_id, vec![Change::create(&new_rel, row.is_dir)]).await
    {
        tracing::warn!("restore {id}: journaling the create failed: {e}");
    }
    lock.release().await?;
    Ok(paths::join(&m.path, &new_rel))
}

/// After a raw item was moved back: cache it, point its share links back
/// (same name only), drop the trash row and folder. Caller holds the lock.
#[allow(clippy::too_many_arguments)]
async fn finish_raw_restore(
    st: &AppState,
    m: &Mount,
    id: i64,
    parent: &str,
    name: &str,
    new_rel: &str,
    same_name: bool,
    dir: &Path,
) -> ApiResult<()> {
    {
        let n = vfs::stat(st, m, new_rel).await?;
        vfs::cache_node(st, m, new_rel, &n).await?;
        if same_name {
            // Links to the item itself follow it back (its cache row is
            // new); links to entries inside a folder stay broken.
            sqlx::query(
                    "UPDATE shares s SET node_id = (SELECT n.id FROM nodes n
                                                   WHERE n.volume_id = $2 AND n.parent = $3 AND n.name = $4)
                     FROM trash_shares t JOIN trash tr ON tr.id = t.trash_id
                     WHERE t.trash_id = $1 AND s.id = t.share_id AND s.node_id IS NULL AND t.node_id = tr.node_id",
                )
                .bind(id)
                .bind(m.volume_id)
                .bind(parent)
                .bind(name)
                .execute(&st.db)
                .await?;
        }
        sqlx::query("DELETE FROM trash WHERE id = $1")
            .bind(id)
            .execute(&st.db)
            .await?;
        let _ = tokio::fs::remove_dir(dir).await;
        sqlx::query("DELETE FROM raw_scans WHERE volume_id = $1 AND (dir = $2 OR dir = $3)")
            .bind(m.volume_id)
            .bind(parent)
            .bind(new_rel)
            .execute(&st.db)
            .await?;
    }
    Ok(())
}

/// Remove a trashed item for good. Hashed blobs are reclaimed later by GC.
/// NotFound if it is gone by the time the volume lock is held (restored or
/// purged meanwhile).
pub async fn purge(st: &AppState, id: i64) -> ApiResult<()> {
    let (volume_id, connection_id, mode): (i64, i64, String) = sqlx::query_as(
        "SELECT v.id, v.connection_id, v.mode FROM trash t JOIN volumes v ON v.id = t.volume_id WHERE t.id = $1",
    )
    .bind(id)
    .fetch_optional(&st.db)
    .await?
    .ok_or(ApiError::NotFound)?;
    let root = if mode == "hashed" {
        None
    } else {
        Some(local_root(st, connection_id).await?)
    };
    let lock = vfs::lock_volume(st, volume_id).await?;
    let still: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM trash WHERE id = $1)")
        .bind(id)
        .fetch_one(&st.db)
        .await?;
    if !still {
        return Err(ApiError::NotFound);
    }
    let mut doomed = None;
    if let Some(root) = &root {
        // Out of the way first, then the row; the files are removed after
        // the lock is released (a big folder must not block the volume). A
        // crash in between leaves a row whose item is gone, which reconcile
        // drops, or a doomed folder, which the sweep removes.
        let to = doomed_dir(root, id);
        match tokio::fs::rename(item_dir(root, id), &to).await {
            Ok(()) => doomed = Some(to),
            // Gone, but only believe it while the trash folder is there: a
            // disk that isn't mounted right now hides it too.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if !trash_reachable(st, connection_id, root).await {
                    return Err(ApiError::Unavailable(
                        "this storage is not available right now; try again later".into(),
                    ));
                }
            }
            Err(e) => return Err(io_error(e)),
        }
    }
    sqlx::query("DELETE FROM trash WHERE id = $1")
        .bind(id)
        .execute(&st.db)
        .await?;
    lock.release().await?;
    if let Some(to) = doomed
        && let Err(e) = tokio::fs::remove_dir_all(&to).await
    {
        tracing::warn!("trash {id}: removing the purged files failed, the sweep will: {e}");
    }
    Ok(())
}

/// Can we trust what we see in a local connection's trash folder? Not while
/// the connection is offline, nor when both the trash folder and the
/// connection's root marker are missing (a disk not mounted right now shows
/// an empty mount point). With the marker there, a missing trash folder was
/// really removed.
async fn trash_reachable(st: &AppState, connection_id: i64, root: &Path) -> bool {
    let is_dir = |p: PathBuf| async move { tokio::fs::metadata(p).await.is_ok_and(|m| m.is_dir()) };
    let marker = crate::state::LocalRoot::marker_path(root, connection_id);
    !st.is_offline(connection_id)
        && (is_dir(root.join(TRASH_DIR)).await || tokio::fs::metadata(marker).await.is_ok())
}

/// Settle raw trash rows after a crash, each under its volume lock (an
/// operation in progress holds it from writing the row to finishing, so it
/// is never mistaken for a crash):
///
/// * not ready, item in the trash folder: the rename happened; mark ready;
/// * not ready, item still in its place: the rename never happened; drop;
/// * item in neither place, or a ready row whose item is gone: drop, but only
///   while the trash folder can be trusted (see `trash_reachable`).
pub async fn reconcile(st: &AppState) -> ApiResult<usize> {
    let rows: Vec<(i64, i64, i64, String, String, String)> = sqlx::query_as(
        "SELECT t.id, t.volume_id, v.connection_id, v.base_path, t.rel_path, t.name
         FROM trash t JOIN volumes v ON v.id = t.volume_id
         WHERE v.mode = 'raw'",
    )
    .fetch_all(&st.db)
    .await?;
    let mut fixed = sweep_orphans(st).await;
    for (id, volume_id, connection_id, base, rel, name) in rows {
        match reconcile_one(st, id, volume_id, connection_id, &base, &rel, &name).await {
            Ok(true) => fixed += 1,
            Ok(false) => {}
            // One broken storage or a DB hiccup must not stop the rest.
            Err(e) => tracing::warn!("trash reconcile of item {id} failed: {e}"),
        }
    }
    Ok(fixed)
}

async fn reconcile_one(
    st: &AppState,
    id: i64,
    volume_id: i64,
    connection_id: i64,
    base: &str,
    rel: &str,
    name: &str,
) -> ApiResult<bool> {
    {
        let Ok(root) = local_root(st, connection_id).await else {
            return Ok(false);
        };
        let lock = vfs::lock_volume(st, volume_id).await?;
        // Re-read: it may have been restored, purged or finished meanwhile.
        let ready: Option<bool> = sqlx::query_scalar("SELECT ready FROM trash WHERE id = $1")
            .bind(id)
            .fetch_optional(&st.db)
            .await?;
        let Some(ready) = ready else {
            lock.release().await?;
            return Ok(false);
        };
        let exists = |p: PathBuf| async move { tokio::fs::symlink_metadata(p).await.is_ok() };
        let item = item_dir(&root, id).join(name);
        let in_trash = exists(item.clone()).await;
        let mut fixed = false;
        let drop_row = if in_trash {
            if !ready {
                if exists(root.join(paths::join(base, rel))).await {
                    // Something new was made at the old place since the
                    // crash: leave its listing and the clients alone.
                    sqlx::query("UPDATE trash SET ready = TRUE WHERE id = $1")
                        .bind(id)
                        .execute(&st.db)
                        .await?;
                } else {
                    let is_dir = tokio::fs::symlink_metadata(&item)
                        .await
                        .is_ok_and(|m| m.is_dir());
                    finish_raw_trash(st, &root, id, volume_id, rel, is_dir).await?;
                }
                fixed = true;
            }
            false
        } else if !ready && exists(root.join(paths::join(base, rel))).await {
            true
        } else {
            trash_reachable(st, connection_id, &root).await
        };
        if drop_row {
            sqlx::query("DELETE FROM trash WHERE id = $1")
                .bind(id)
                .execute(&st.db)
                .await?;
            let _ = tokio::fs::remove_dir(item_dir(&root, id)).await;
            fixed = true;
        }
        lock.release().await?;
        Ok(fixed)
    }
}

/// Tidy the trash folders of local connections: remove what a purge left
/// doomed, and set aside `.strata-trash/<id>` folders that no row knows
/// (their owner was deleted while the item was moving, or the database was
/// restored from a backup). A row is always written before its folder is
/// made, so a folder without a row is never one being filled. A folder is
/// set aside only when the previous sweep (an hour earlier) saw it without a
/// row too: a user or connection delete removes the rows a moment before the
/// folders. Set-aside folders keep their files (`orphan-<id>-<time>`) for an
/// admin to look at. Best effort; returns how many folders were handled.
async fn sweep_orphans(st: &AppState) -> usize {
    let Ok(conns) = sqlx::query_scalar::<_, i64>("SELECT id FROM connections WHERE kind = 'local'")
        .fetch_all(&st.db)
        .await
    else {
        return 0;
    };
    static SEEN: std::sync::Mutex<BTreeSet<PathBuf>> = std::sync::Mutex::new(BTreeSet::new());
    let seen = || SEEN.lock().unwrap_or_else(|e| e.into_inner());
    let mut handled = 0;
    for cid in conns {
        let Ok(root) = local_root(st, cid).await else {
            continue;
        };
        let trash = root.join(TRASH_DIR);
        // What the last sweep of this folder found unknown (kept per folder).
        let before: BTreeSet<PathBuf> = {
            let mut all = seen();
            let mine: Vec<PathBuf> = all
                .iter()
                .filter(|p| p.starts_with(&trash))
                .cloned()
                .collect();
            mine.into_iter().filter(|p| all.remove(p)).collect()
        };
        let mut now = Vec::new();
        let Ok(mut rd) = tokio::fs::read_dir(&trash).await else {
            continue;
        };
        while let Ok(Some(e)) = rd.next_entry().await {
            let name = e.file_name();
            let Some(name) = name.to_str() else { continue };
            let doomed = name.strip_prefix("purging-").is_some_and(|rest| {
                rest.split_once('-')
                    .is_some_and(|(a, b)| a.parse::<i64>().is_ok() && b.parse::<i64>().is_ok())
            });
            if doomed {
                if tokio::fs::remove_dir_all(e.path()).await.is_ok() {
                    handled += 1;
                }
                continue;
            }
            let Ok(id) = name.parse::<i64>() else {
                continue;
            };
            let known: bool =
                match sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM trash WHERE id = $1)")
                    .bind(id)
                    .fetch_one(&st.db)
                    .await
                {
                    Ok(k) => k,
                    Err(_) => continue,
                };
            if known {
                continue;
            }
            if !before.contains(&e.path()) {
                now.push(e.path());
                continue;
            }
            match set_aside(&root, id).await {
                Ok(()) => {
                    tracing::warn!(
                        "trash folder {id} of connection {cid} has no record; set aside as orphan-{id}-*"
                    );
                    handled += 1;
                }
                Err(e) => tracing::warn!("setting aside trash folder {id} failed: {e}"),
            }
        }
        seen().extend(now);
    }
    handled
}

/// Drop a trash row whose storage can't be reached, removing its files if
/// they happen to be there (best effort). For deleting a connection only.
pub async fn forget(st: &AppState, id: i64) -> ApiResult<()> {
    let conn: Option<(i64, String)> = sqlx::query_as(
        "DELETE FROM trash t USING volumes v WHERE v.id = t.volume_id AND t.id = $1
         RETURNING v.connection_id, v.mode",
    )
    .bind(id)
    .fetch_optional(&st.db)
    .await?;
    if let Some((connection_id, mode)) = conn
        && mode == "raw"
    {
        remove_raw_items(st, &[(id, connection_id)]).await;
    }
    Ok(())
}

/// Remove raw trash items (id, connection) whose rows are already gone.
/// Best effort: a failure is logged and leaves only hidden files behind.
pub async fn remove_raw_items(st: &AppState, items: &[(i64, i64)]) {
    for &(id, connection_id) in items {
        let Ok(root) = local_root(st, connection_id).await else {
            continue;
        };
        remove_items_at(&root, &[id]).await;
    }
}

/// The folder of a local connection (for removing its trash after the
/// connection itself is gone).
pub(crate) async fn connection_root(st: &AppState, connection_id: i64) -> ApiResult<PathBuf> {
    local_root(st, connection_id).await
}

/// Remove raw trash items under a known connection folder. Best effort.
pub(crate) async fn remove_items_at(root: &Path, ids: &[i64]) {
    for &id in ids {
        match tokio::fs::remove_dir_all(item_dir(root, id)).await {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => tracing::warn!("removing trash item {id} failed: {e}"),
        }
    }
}

/// Purge everything past its retention date. Returns how many were purged.
pub async fn purge_expired(st: &AppState) -> ApiResult<usize> {
    let ids: Vec<i64> =
        sqlx::query_scalar("SELECT id FROM trash WHERE purge_after < now() ORDER BY id")
            .fetch_all(&st.db)
            .await?;
    let mut n = 0;
    for id in ids {
        match purge(st, id).await {
            Ok(()) => n += 1,
            Err(ApiError::NotFound) => {} // restored or purged meanwhile
            // One broken storage must not stop the rest.
            Err(e) => tracing::warn!("purging trash item {id} failed: {e}"),
        }
    }
    Ok(n)
}

// ---------------------------------------------------------------- HTTP

#[derive(Deserialize)]
pub struct ListQuery {
    pub limit: Option<i64>,
    /// Items with a smaller id than this (the last id of the previous page).
    pub before: Option<i64>,
}

/// `GET /api/trash`: newest first, in pages.
pub async fn list(
    State(st): State<AppState>,
    AuthUser(u): AuthUser,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let limit = q.limit.unwrap_or(200).clamp(1, 1000);
    let items: Vec<Item> = sqlx::query_as(
        "SELECT id, name, original_path, is_dir, size, deleted_at, purge_after FROM trash
         WHERE owner_id = $1 AND ready AND ($2::bigint IS NULL OR id < $2)
         ORDER BY id DESC LIMIT $3",
    )
    .bind(u.id)
    .bind(q.before)
    .bind(limit + 1)
    .fetch_all(&st.db)
    .await?;
    let mut items = items;
    let more = items.len() as i64 > limit;
    items.truncate(limit as usize);
    Ok(Json(json!({
        "retention_days": st.cfg.trash_days,
        "items": items,
        "more": more,
    })))
}

/// `POST /api/trash/{id}/restore`
pub async fn restore_one(
    State(st): State<AppState>,
    AuthUser(u): AuthUser,
    UrlPath(id): UrlPath<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    let path = restore(&st, u.id, id).await?;
    Ok(Json(json!({ "path": path })))
}

/// `DELETE /api/trash/{id}`
pub async fn delete_one(
    State(st): State<AppState>,
    AuthUser(u): AuthUser,
    UrlPath(id): UrlPath<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    let row = owned_row(&st, u.id, id).await?;
    purge(&st, row.id).await?;
    Ok(Json(json!({})))
}

/// `POST /api/trash/empty`: purge all of the user's items.
pub async fn empty(
    State(st): State<AppState>,
    AuthUser(u): AuthUser,
) -> ApiResult<Json<serde_json::Value>> {
    let ids: Vec<i64> =
        sqlx::query_scalar("SELECT id FROM trash WHERE owner_id = $1 AND ready ORDER BY id")
            .bind(u.id)
            .fetch_all(&st.db)
            .await?;
    let mut purged = 0;
    let mut failed = 0;
    for id in ids {
        match purge(&st, id).await {
            Ok(()) => purged += 1,
            Err(ApiError::NotFound) => {}
            Err(e) => {
                tracing::warn!("emptying trash: item {id} failed: {e}");
                failed += 1;
            }
        }
    }
    // Some may fail (storage offline): they stay, and the answer says so.
    Ok(Json(json!({ "purged": purged, "failed": failed })))
}

#[cfg(test)]
mod tests {
    use super::restored_name;

    #[test]
    fn restored_names() {
        assert_eq!(restored_name("a.txt", false, 1), "a (restored).txt");
        assert_eq!(restored_name("a.txt", false, 2), "a (restored 2).txt");
        assert_eq!(restored_name("photos", true, 1), "photos (restored)");
        assert_eq!(restored_name("v1.2", true, 1), "v1.2 (restored)");
        assert_eq!(restored_name(".env", false, 1), ".env (restored)");
        assert_eq!(restored_name("a.tar.gz", false, 3), "a.tar (restored 3).gz");
    }
}
