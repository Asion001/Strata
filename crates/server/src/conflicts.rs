//! What a move, copy or upload does when its destination name is taken:
//! fail (409 `exists`, so the client can ask), replace what is there, or
//! keep both under a free "name (2).ext".
//!
//! Names being written are claimed in memory for the whole operation, so
//! two operations of this server never pick the same name (two "Keep both"
//! end up with two names, never one overwriting the other). A write from
//! outside Strata meanwhile is still caught by the operations' own checks
//! (a move refuses an existing destination); nothing here overwrites.
//!
//! Replace never removes the old item before the new one is complete. An
//! upload over a file saves new content into that same file, atomically (so
//! it keeps its share links and history). Anything else is written beside
//! the old item under a hidden temporary name, then the old one is deleted
//! (to the trash where there is one, with its share links: a different item
//! never inherits them), then the new one takes its name. The hidden item is
//! recorded until then, so one a crash leaves behind is settled by the
//! background job (`reconcile`). Every step runs to its end even if the
//! request goes away (`detached`).

use serde::Deserialize;

use crate::auth::random_token;
use crate::error::{ApiError, ApiResult, Existing, bad, conflict};
use crate::state::AppState;
use crate::{mounts, vfs};
use strata_storage::paths;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum OnConflict {
    /// Answer 409 with `code: "exists"` and what is there.
    #[default]
    Fail,
    /// Put the new item in place of the old one. An upload over a file
    /// saves into that file (its share links stay); anything else sends the
    /// old item to the trash where there is one.
    Replace,
    /// Keep both: the new item gets a free "name (2).ext".
    Rename,
}

impl OnConflict {
    pub fn as_str(self) -> &'static str {
        match self {
            OnConflict::Fail => "fail",
            OnConflict::Replace => "replace",
            OnConflict::Rename => "rename",
        }
    }

    /// As stored by `as_str` (anything else: the default).
    pub fn parse(s: &str) -> Self {
        match s {
            "replace" => OnConflict::Replace,
            "rename" => OnConflict::Rename,
            _ => OnConflict::Fail,
        }
    }
}

/// Names of items a replace writes beside the one it replaces (`.strata-`
/// and a token), as a regular expression: left out of listings and search.
pub const STAGED_NAME: &str = r"^\.strata-[0-9a-f]{18}$";
/// The same, as any part of a path (what is inside one too).
pub const STAGED_PART: &str = r"(^|/)\.strata-[0-9a-f]{18}(/|$)";

/// Whether any part of `path` is a staged item's name (see `STAGED_NAME`).
pub fn is_staged(path: &str) -> bool {
    path.split('/').any(|c| {
        c.strip_prefix(".strata-").is_some_and(|t| {
            t.len() == 18 && t.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        })
    })
}

/// A name claimed for an operation in flight (released when dropped).
pub struct Claim {
    st: AppState,
    key: String,
}

impl Drop for Claim {
    fn drop(&mut self) {
        self.st.claims.remove(&self.key);
    }
}

/// Claim `rel` of `m`, keyed by where it is in storage (two raw mounts can
/// show the same folder of one connection).
fn claim(st: &AppState, m: &mounts::Mount, rel: &str) -> Option<Claim> {
    claim_key(st, claim_key_of(m, rel))
}

fn claim_key_of(m: &mounts::Mount, rel: &str) -> String {
    if m.is_hashed() {
        format!("v{}:{rel}", m.volume_id)
    } else {
        format!("c{}:{}", m.connection_id, paths::join(&m.base_path, rel))
    }
}

/// Claim `key` once the operation holding it is done (`None` if it takes
/// longer than `busy_wait_ms`).
pub async fn wait_claim(st: &AppState, key: String) -> Option<Claim> {
    let now = tokio::time::Instant::now();
    let until = now
        .checked_add(std::time::Duration::from_millis(st.cfg.busy_wait_ms))
        .unwrap_or(now + std::time::Duration::from_secs(86400));
    loop {
        if let Some(c) = claim_key(st, key.clone()) {
            return Some(c);
        }
        if tokio::time::Instant::now() >= until {
            return None;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

/// Refuse (for now: 503) to move, copy or delete `path` while an operation
/// of this server writes inside it: it would carry off, copy or remove an
/// item mid-write.
pub async fn check_not_busy_inside(st: &AppState, user_id: i64, path: &str) -> ApiResult<()> {
    let (m, rel) = mounts::resolve(st, user_id, path).await?;
    let key = claim_key_of(&m, &rel);
    let inside = if key.ends_with(':') { key } else { key + "/" };
    if st.claims.iter().any(|c| c.key().starts_with(&inside)) {
        return Err(ApiError::Unavailable(
            "something inside is being written; try again".into(),
        ));
    }
    Ok(())
}

/// Claim anything named by `key` (an upload being committed, say).
pub fn claim_key(st: &AppState, key: String) -> Option<Claim> {
    use dashmap::mapref::entry::Entry;
    match st.claims.entry(key.clone()) {
        Entry::Occupied(_) => None,
        Entry::Vacant(v) => {
            v.insert(());
            Some(Claim {
                st: st.clone(),
                key,
            })
        }
    }
}

/// "a.txt" → "a (2).txt"; a folder or a leading-dot name keeps its whole
/// name before the number ("v1.2 (2)", ".env (2)"). A number already in the
/// name stays: it may be the user's own ("Report (2019) (2).pdf").
pub fn numbered(name: &str, is_dir: bool, n: u32) -> String {
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 && !is_dir => name.split_at(i),
        _ => (name, ""),
    };
    format!("{stem} ({n}){ext}")
}

/// Where an operation may write, claimed.
pub struct Target {
    /// In the user's tree.
    pub path: String,
    /// What is there now (only with `Replace`).
    pub existing: bool,
    _claim: Claim,
}

/// The place for an item arriving at `to` (a user path), by `mode`.
pub async fn place(
    st: &AppState,
    user_id: i64,
    to: &str,
    is_dir: bool,
    mode: OnConflict,
) -> ApiResult<Target> {
    let (parent, name) = paths::split(to).ok_or_else(|| bad("a name is required"))?;
    let all = mounts::user_mounts(st, user_id).await?;
    for n in 1..1000u32 {
        let candidate = if n == 1 {
            to.to_string()
        } else {
            let name = numbered(name, is_dir, n);
            if name.len() > 255 {
                return Err(conflict(
                    "the name is too long to add a number; rename one of them, then try again",
                ));
            }
            paths::join(parent, &name)
        };
        // Something mounted there: never written over, never replaced.
        if all.iter().any(|m| m.path == candidate) {
            if mode == OnConflict::Rename {
                continue;
            }
            return Err(conflict("something is mounted there"));
        }
        let (m, rel) = mounts::resolve(st, user_id, &candidate).await?;
        let c = match claim(st, &m, &rel) {
            Some(c) => c,
            // Another operation is writing this name right now.
            None if mode == OnConflict::Rename => continue,
            // Its result is what this one replaces or asks about: after it.
            None => {
                let key = claim_key_of(&m, &rel);
                wait_claim(st, key)
                    .await
                    // (Not a refusal: the other one is still at it.)
                    .ok_or_else(|| {
                        ApiError::Unavailable(
                            "something is still being written there; try again".into(),
                        )
                    })?
            }
        };
        let node = match vfs::stat(st, &m, &rel).await {
            Err(ApiError::NotFound) => {
                return Ok(Target {
                    path: candidate,
                    existing: false,
                    _claim: c,
                });
            }
            Err(e) => return Err(e),
            Ok(node) => node,
        };
        match mode {
            OnConflict::Rename => continue,
            OnConflict::Replace if node.is_dir != is_dir => {
                return Err(conflict(if node.is_dir {
                    "a file can't replace a folder; keep both instead"
                } else {
                    "a folder can't replace a file; keep both instead"
                }));
            }
            OnConflict::Replace => {
                return Ok(Target {
                    path: candidate,
                    existing: true,
                    _claim: c,
                });
            }
            OnConflict::Fail => {
                return Err(ApiError::Exists(Box::new(Existing {
                    name: node.name.clone(),
                    path: candidate,
                    is_dir: node.is_dir,
                    size: node.size,
                    mtime: node.mtime,
                })));
            }
        }
    }
    Err(conflict("too many items with that name"))
}

/// Run `op` to its end even if the request that started it goes away: a
/// replace dropped halfway would leave the item under its hidden name.
pub async fn detached<T, Fut>(op: Fut) -> ApiResult<T>
where
    T: Send + 'static,
    Fut: std::future::Future<Output = ApiResult<T>> + Send + 'static,
{
    tokio::spawn(op)
        .await
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("operation stopped: {e}")))?
}

/// Where the arriving item comes from.
#[derive(Clone, Copy)]
pub enum Source<'a> {
    /// Moved from this user path (a failed move may have moved part of it).
    Move(&'a str),
    /// Copied from this user path.
    Copy(&'a str),
    /// New bytes (an upload).
    Upload,
}

/// Run `write` (which puts the new item at the path it is given) for an item
/// arriving at `to`, by `mode`. Returns where it landed.
///
/// `write` is told whether it is saving over a file in place: only an
/// upload replacing a file is (the old behavior of a save: the same file,
/// with its share links and history, gets new content atomically). Anything
/// else written where something is goes beside it first.
pub async fn write_at<F, Fut>(
    st: &AppState,
    user_id: i64,
    to: &str,
    is_dir: bool,
    mode: OnConflict,
    source: Source<'_>,
    write: F,
) -> ApiResult<String>
where
    F: Fn(String, bool) -> Fut,
    Fut: std::future::Future<Output = ApiResult<()>>,
{
    let target = place(st, user_id, to, is_dir, mode).await?;
    if !target.existing {
        write(target.path.clone(), false).await?;
        return Ok(target.path);
    }
    let from = match source {
        Source::Move(f) | Source::Copy(f) => f,
        // (An upload is a file, and `place` refused one over a folder.)
        Source::Upload => {
            write(target.path.clone(), true).await?;
            return Ok(target.path);
        }
    };
    // (A copy would delete its own source; a move would empty the folder it
    // replaces first, which is surprising at best. Compared in storage: two
    // mounts can show the same folder.)
    let (tm, trel) = mounts::resolve(st, user_id, &target.path).await?;
    let (fm, frel) = mounts::resolve(st, user_id, from).await?;
    if vfs::contains(&tm, &trel, &fm, &frel) {
        return Err(conflict(if vfs::contains(&fm, &frel, &tm, &trel) {
            "it is already there (the same item, through another mount)"
        } else {
            "it can't replace the folder it is in; pick another folder or keep both"
        }));
    }
    // Nothing mounted inside what goes: checked now, not after the whole
    // item was written beside it.
    if is_dir
        && mounts::user_mounts(st, user_id)
            .await?
            .iter()
            .any(|m| paths::strip_prefix(&m.path, &target.path).is_some())
    {
        return Err(conflict(
            "something is mounted inside it; keep both, or unmount it first",
        ));
    }
    // Beside the old one first, under a hidden name of its own, recorded so
    // a crash midway is settled later (`reconcile`).
    let (parent, _) = paths::split(&target.path).unwrap();
    let staged = place(
        st,
        user_id,
        // (Short, so a long name still fits.)
        &paths::join(parent, &format!(".strata-{}", random_token(9))),
        is_dir,
        OnConflict::Fail,
    )
    .await?;
    let row: i64 = sqlx::query_scalar(
        "INSERT INTO staged_items (user_id, path, target, is_dir, is_move)
         VALUES ($1, $2, $3, $4, $5) RETURNING id",
    )
    .bind(user_id)
    .bind(&staged.path)
    .bind(&target.path)
    .bind(is_dir)
    .bind(matches!(source, Source::Move(_)))
    .fetch_one(&st.db)
    .await?;
    // (Alive while it runs: no server settles it meanwhile.)
    let _beat = Heartbeat(tokio::spawn({
        let db = st.db.clone();
        async move {
            let mut every = tokio::time::interval(HEARTBEAT);
            every.tick().await;
            loop {
                every.tick().await;
                let _ = sqlx::query("UPDATE staged_items SET alive_at = now() WHERE id = $1")
                    .bind(row)
                    .execute(&db)
                    .await;
            }
        }
    }));
    let r = replace(
        st,
        row,
        user_id,
        &target.path,
        &staged.path,
        is_dir,
        source,
        from,
        write,
    )
    .await;
    // Settled, unless the staged item may still be there (a cleanup that
    // failed, storage that can't say): then `reconcile` tries again.
    if !maybe_there(st, user_id, &staged.path).await
        && let Err(e) = sqlx::query("DELETE FROM staged_items WHERE id = $1")
            .bind(row)
            .execute(&st.db)
            .await
    {
        // (Harmless: `reconcile` drops a record whose item is gone.)
        tracing::warn!("staged item record {row} not removed: {e}");
    }
    r.map(|()| target.path.clone())
}

/// Replace what is at `to` with the item `write` puts at `staged`.
#[allow(clippy::too_many_arguments)]
async fn replace<F, Fut>(
    st: &AppState,
    row: i64,
    user_id: i64,
    to: &str,
    staged: &str,
    is_dir: bool,
    source: Source<'_>,
    from: &str,
    write: F,
) -> ApiResult<()>
where
    F: Fn(String, bool) -> Fut,
    Fut: std::future::Future<Output = ApiResult<()>>,
{
    let name = paths::split(to).unwrap().1;
    let is_move = matches!(source, Source::Move(_));
    if let Err(e) = write(staged.to_string(), false).await {
        // The old item is untouched. A partial copy goes; part of a move is
        // the user's only copy of it: shown, beside the old one. (Only if it
        // is surely there: otherwise the error says what happened, and the
        // record stays for `reconcile`.)
        if is_move {
            if is_there(st, user_id, staged).await {
                e.log_internal();
                let kept = kept_where(st, user_id, staged, to, is_dir).await;
                return Err(conflict(format!(
                    "{}; what was moved is in {kept}",
                    e.public_message()
                )));
            }
        } else {
            let _ = vfs::delete(st, user_id, staged, false).await;
        }
        return Err(e);
    }
    // Whole now (recorded first: after a crash past this point, a copy may
    // be all there is). Only now does the old one go (to the trash where
    // there is one). Its share links stay with it: a different item at the
    // same path never inherits them.
    let marked = sqlx::query("UPDATE staged_items SET written = TRUE WHERE id = $1")
        .bind(row)
        .execute(&st.db)
        .await;
    let gone = match marked {
        // (Gone already, deleted meanwhile: the name is free all the same.)
        Ok(_) => match vfs::delete(st, user_id, to, true).await {
            Err(ApiError::NotFound) => Ok(()),
            r => r.map(|_| ()),
        },
        Err(e) => Err(e.into()),
    };
    if let Err(e) = gone {
        // The old one stays; so does the source of a move, which is now
        // the staged item: put it back where it was.
        if is_move {
            if vfs::move_path(st, user_id, staged, from).await.is_err() {
                e.log_internal();
                return Err(conflict(format!(
                    "couldn't replace “{name}” ({}); the item is kept as {}",
                    e.public_message(),
                    kept_where(st, user_id, staged, to, is_dir).await
                )));
            }
        } else {
            let _ = vfs::delete(st, user_id, staged, false).await;
        }
        return Err(e);
    }
    // (Once more if the first try fails: the name is free now, and the new
    // item belongs there.)
    let mut moved = vfs::move_path(st, user_id, staged, to).await;
    if moved.is_err() {
        moved = vfs::move_path(st, user_id, staged, to).await;
    }
    if let Err(e) = moved {
        e.log_internal();
        // (Never `to` itself: this operation still holds that name.)
        return Err(conflict(format!(
            "the old “{name}” was deleted, but the new one couldn't take its name ({}); it is kept as {}",
            e.public_message(),
            kept_where(st, user_id, staged, to, is_dir).await
        )));
    }
    Ok(())
}

/// Settle what crashed replaces left (see `staged_items`): part of a move
/// is shown beside the item it was replacing (or in its place, if that one
/// went already); part of a copy goes. Items an operation (of any server)
/// is still writing are left alone. Returns how many were settled.
pub async fn reconcile(st: &AppState) -> ApiResult<usize> {
    let rows: Vec<(i64, i64, String, String, bool, bool, bool)> = sqlx::query_as(
        "SELECT id, user_id, path, target, is_dir, is_move, written FROM staged_items
         WHERE alive_at < now() - make_interval(secs => $1) ORDER BY id",
    )
    .bind(STALE_SECS as f64)
    .fetch_all(&st.db)
    .await?;
    let mut settled = 0;
    for (id, user_id, path, target, is_dir, is_move, written) in rows {
        // (A path that no longer resolves, or storage that can't be read
        // now: next time.)
        let Ok((m, rel)) = mounts::resolve(st, user_id, &path).await else {
            continue;
        };
        let Some(_c) = claim(st, &m, &rel) else {
            continue;
        };
        let done = match vfs::stat(st, &m, &rel).await {
            Err(ApiError::NotFound) => true,
            Err(_) => false,
            Ok(_) if is_move => keep_visible(st, user_id, &path, &target, is_dir).await != path,
            // Part of a copy goes (its source is still there), unless it is
            // whole and the old item went already (a crash just before the
            // rename): then it is the only copy of what was there.
            Ok(_) => match maybe_there(st, user_id, &target).await {
                false if written => keep_visible(st, user_id, &path, &target, is_dir).await != path,
                true if !is_there(st, user_id, &target).await => false,
                _ => vfs::delete(st, user_id, &path, false).await.is_ok(),
            },
        };
        if done {
            sqlx::query("DELETE FROM staged_items WHERE id = $1")
                .bind(id)
                .execute(&st.db)
                .await?;
            settled += 1;
        }
    }
    Ok(settled)
}

/// Stops a running replace's heartbeat when the replace ends (or unwinds).
struct Heartbeat(tokio::task::JoinHandle<()>);

impl Drop for Heartbeat {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Whether something is surely at `path`.
async fn is_there(st: &AppState, user_id: i64, path: &str) -> bool {
    match mounts::resolve(st, user_id, path).await {
        Ok((m, rel)) => vfs::stat(st, &m, &rel).await.is_ok(),
        Err(_) => false,
    }
}

/// Whether something may be at `path`: only a clear "not found" says no.
async fn maybe_there(st: &AppState, user_id: i64, path: &str) -> bool {
    let r = match mounts::resolve(st, user_id, path).await {
        Ok((m, rel)) => vfs::stat(st, &m, &rel).await.map(|_| ()),
        Err(e) => Err(e),
    };
    !matches!(r, Err(ApiError::NotFound))
}

/// How often a running replace says it is alive, and how long after the
/// last time `reconcile` takes it for interrupted.
const HEARTBEAT: std::time::Duration = std::time::Duration::from_secs(60);
const STALE_SECS: i64 = 300;

/// `keep_visible`, said for people: a hidden name is not somewhere they can
/// go (the background job shows it later).
async fn kept_where(st: &AppState, user_id: i64, staged: &str, to: &str, is_dir: bool) -> String {
    let kept = keep_visible(st, user_id, staged, to, is_dir).await;
    if kept == staged {
        format!(
            "a hidden place for now, and will appear beside “{}” within the hour",
            paths::split(to).map_or(to, |(_, n)| n)
        )
    } else {
        kept
    }
}

/// Give a staged item that has to stay a visible "name (2)" beside `to`
/// (best effort: its hidden name if that fails). Returns where it is.
async fn keep_visible(st: &AppState, user_id: i64, staged: &str, to: &str, is_dir: bool) -> String {
    let Ok(free) = place(st, user_id, to, is_dir, OnConflict::Rename).await else {
        return staged.to_string();
    };
    // (The claim on the free name is held through the move.)
    match vfs::move_path(st, user_id, staged, &free.path).await {
        Ok(()) => free.path.clone(),
        Err(_) => staged.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbered_names() {
        assert_eq!(numbered("a.txt", false, 2), "a (2).txt");
        assert_eq!(numbered("a.tar.gz", false, 3), "a.tar (3).gz");
        assert_eq!(numbered("photos", true, 2), "photos (2)");
        assert_eq!(numbered("v1.2", true, 2), "v1.2 (2)");
        assert_eq!(numbered(".env", false, 2), ".env (2)");
        assert_eq!(numbered("noext", false, 4), "noext (4)");
        assert_eq!(
            numbered("Report (2019).pdf", false, 2),
            "Report (2019) (2).pdf"
        );
        assert_eq!(numbered("photos (12)", true, 2), "photos (12) (2)");
    }
}
