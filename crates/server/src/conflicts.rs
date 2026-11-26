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
//! never inherits them), then the new one takes its name.

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
    let key = if m.is_hashed() {
        format!("v{}:{rel}", m.volume_id)
    } else {
        format!("c{}:{}", m.connection_id, paths::join(&m.base_path, rel))
    };
    claim_key(st, key)
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
/// name before the number ("v1.2 (2)", ".env (2)"). A number already there
/// is replaced, not added to ("a (2).txt" → "a (3).txt").
pub fn numbered(name: &str, is_dir: bool, n: u32) -> String {
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 && !is_dir => name.split_at(i),
        _ => (name, ""),
    };
    let stem = stem
        .strip_suffix(')')
        .and_then(|s| s.rsplit_once(" ("))
        .filter(|(base, num)| {
            !base.is_empty() && !num.is_empty() && num.bytes().all(|b| b.is_ascii_digit())
        })
        .map_or(stem, |(base, _)| base);
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
        let Some(c) = claim(st, &m, &rel) else {
            // Another operation is writing this name right now.
            if mode == OnConflict::Rename {
                continue;
            }
            return Err(conflict("something is being written there; try again"));
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
        return Err(conflict(
            "it can't replace the folder it is in; pick another folder or keep both",
        ));
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
    // Beside the old one first, under a hidden name of its own.
    let (parent, name) = paths::split(&target.path).unwrap();
    let staged = place(
        st,
        user_id,
        // (Short, so a long name still fits.)
        &paths::join(parent, &format!(".strata-{}", random_token(9))),
        is_dir,
        OnConflict::Fail,
    )
    .await?;
    if let Err(e) = write(staged.path.clone(), false).await {
        // The old item is untouched. A partial copy goes; part of a move is
        // the user's only copy of it: shown, beside the old one.
        if matches!(source, Source::Move(_)) {
            if exists(st, user_id, &staged.path).await {
                let kept = keep_visible(st, user_id, &staged.path, &target.path, is_dir).await;
                return Err(conflict(format!(
                    "{}; what was moved is in {kept}",
                    e.public_message()
                )));
            }
        } else {
            let _ = vfs::delete(st, user_id, &staged.path, false).await;
        }
        return Err(e);
    }
    // Only now does the old one go (to the trash where there is one). Its
    // share links stay with it: a different item at the same path never
    // inherits them.
    if let Err(e) = vfs::delete(st, user_id, &target.path, true).await {
        // The old one stays; so does the source of a move, which is now
        // the staged item: put it back where it was.
        if matches!(source, Source::Move(_)) {
            if vfs::move_path(st, user_id, &staged.path, from)
                .await
                .is_err()
            {
                return Err(conflict(format!(
                    "couldn't replace “{name}” ({}); the item is kept as {}",
                    e.public_message(),
                    keep_visible(st, user_id, &staged.path, &target.path, is_dir).await
                )));
            }
        } else {
            let _ = vfs::delete(st, user_id, &staged.path, false).await;
        }
        return Err(e);
    }
    // (Once more if the first try fails: the name is free now, and the new
    // item belongs there.)
    let mut moved = vfs::move_path(st, user_id, &staged.path, &target.path).await;
    if moved.is_err() {
        moved = vfs::move_path(st, user_id, &staged.path, &target.path).await;
    }
    if let Err(e) = moved {
        // (Never `to` itself: this operation still holds that name.)
        return Err(conflict(format!(
            "the old “{name}” was deleted, but the new one couldn't take its name ({}); it is kept as {}",
            e.public_message(),
            keep_visible(st, user_id, &staged.path, &target.path, is_dir).await
        )));
    }
    Ok(target.path)
}

async fn exists(st: &AppState, user_id: i64, path: &str) -> bool {
    match mounts::resolve(st, user_id, path).await {
        Ok((m, rel)) => vfs::stat(st, &m, &rel).await.is_ok(),
        Err(_) => false,
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
        assert_eq!(numbered("a (2).txt", false, 3), "a (3).txt");
        assert_eq!(numbered("photos (12)", true, 2), "photos (2)");
        assert_eq!(numbered("(2)", true, 3), "(2) (3)");
        assert_eq!(numbered("a (x).txt", false, 2), "a (x) (2).txt");
        assert_eq!(numbered("a ().txt", false, 2), "a () (2).txt");
    }
}
