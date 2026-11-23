//! Background jobs: blob garbage collection, upload/staging cleanup and
//! connection health probes (`health`).

use std::time::Duration;

use strata_storage::Hash;

use crate::mounts::Mount;
use crate::state::AppState;
use crate::vfs;

pub fn spawn(st: AppState) {
    tokio::spawn(crate::health::run(st.clone()));
    tokio::spawn(async move {
        let mut t = tokio::time::interval(Duration::from_secs(3600));
        loop {
            t.tick().await;
            if let Err(e) = crate::journal::prune(&st).await {
                tracing::warn!("journal prune failed: {e}");
            }
            match gc(&st, st.cfg.gc_grace_hours).await {
                Ok(n) if n > 0 => tracing::info!("gc removed {n} blobs"),
                Ok(_) => {}
                Err(e) => tracing::warn!("gc failed: {e:#}"),
            }
        }
    });
}

/// Delete blobs no file or pending upload references, older than the grace
/// period. Returns the number of blobs removed.
pub async fn gc(st: &AppState, grace_hours: i64) -> anyhow::Result<usize> {
    // Expired uploads first, so their chunks become collectable.
    sqlx::query("DELETE FROM uploads WHERE created_at < now() - interval '2 days'")
        .execute(&st.db)
        .await?;
    let volumes: Vec<(i64, i64, String)> =
        sqlx::query_as("SELECT id, connection_id, base_path FROM volumes WHERE mode = 'hashed'")
            .fetch_all(&st.db)
            .await?;
    let mut removed = 0;
    for (vid, cid, base) in volumes {
        match gc_volume(st, vid, cid, &base, grace_hours).await {
            Ok(n) => removed += n,
            // One unreachable backend must not stop GC elsewhere.
            Err(e) => tracing::warn!("gc of volume {vid} failed: {e:#}"),
        }
    }
    clean_dir(&st.staging, Duration::from_secs(2 * 86400)).await;
    // Leftovers of interrupted atomic writes on local connections.
    let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM connections WHERE kind = 'local'")
        .fetch_all(&st.db)
        .await?;
    for id in ids {
        if let Ok(cfg) = st.connection_config(id).await
            && let Ok(Some(dir)) = cfg.local_dir(&st.cfg.local_base)
        {
            clean_dir(
                &dir.join(strata_storage::backend::ATOMIC_TMP_DIR),
                Duration::from_secs(86400),
            )
            .await;
        }
    }
    Ok(removed)
}

/// Collect one hashed volume, in two steps per blob so a crash at any point
/// loses nothing that is still wanted:
///
/// 1. mark the row `deleting` (only if old and unreferenced), commit;
/// 2. lock the row, re-check it is still `deleting`, old and unreferenced,
///    delete the object, then the row, commit.
///
/// A writer that meets a `deleting` row writes the object again and clears the
/// flag (`vfs::store_blob`), so step 2 then skips it. If the object delete
/// fails, the row stays and the next run retries.
async fn gc_volume(
    st: &AppState,
    vid: i64,
    cid: i64,
    base: &str,
    grace_hours: i64,
) -> anyhow::Result<usize> {
    const UNREFERENCED: &str = "b.volume_id = $1 AND b.created_at < now() - make_interval(hours => $2)
           AND NOT EXISTS (SELECT 1 FROM nodes n WHERE n.volume_id = $1 AND n.chunks @> ARRAY[b.hash])
           AND NOT EXISTS (SELECT 1 FROM uploads u WHERE u.volume_id = $1 AND u.chunks @> ARRAY[b.hash])";
    let marked: Vec<Vec<u8>> = sqlx::query_scalar(&format!(
        "UPDATE blobs b SET deleting = TRUE
         WHERE (b.volume_id, b.hash) IN (SELECT b.volume_id, b.hash FROM blobs b WHERE {UNREFERENCED} LIMIT 10000)
         RETURNING b.hash"
    ))
    .bind(vid)
    .bind(grace_hours as i32)
    .fetch_all(&st.db)
    .await?;
    if marked.is_empty() {
        return Ok(0);
    }
    let store = vfs::hashed_store(st, &fake_mount(vid, cid, base))
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut removed = 0;
    for h in marked {
        let Some(hash) = Hash::from_slice(&h) else {
            continue;
        };
        let mut tx = st.db.begin().await?;
        let still: Option<Vec<u8>> = sqlx::query_scalar(&format!(
            "SELECT b.hash FROM blobs b WHERE {UNREFERENCED} AND b.hash = $3 AND b.deleting FOR UPDATE"
        ))
        .bind(vid)
        .bind(grace_hours as i32)
        .bind(&h)
        .fetch_optional(&mut *tx)
        .await?;
        if still.is_none() {
            // Wanted again. Under the row lock, clear the flag only if the
            // object is really there (an earlier delete may have half-failed);
            // otherwise leave it and the next write (store_blob) rewrites it.
            let locked: Option<bool> = sqlx::query_scalar(
                "SELECT deleting FROM blobs WHERE volume_id = $1 AND hash = $2 FOR UPDATE",
            )
            .bind(vid)
            .bind(&h)
            .fetch_optional(&mut *tx)
            .await?;
            if locked == Some(true) && matches!(store.exists(&hash).await, Ok(true)) {
                sqlx::query("UPDATE blobs SET deleting = FALSE WHERE volume_id = $1 AND hash = $2")
                    .bind(vid)
                    .bind(&h)
                    .execute(&mut *tx)
                    .await?;
                tx.commit().await?;
            }
            continue;
        }
        if let Err(e) = store.delete(&hash).await {
            tracing::warn!("gc: deleting blob {hash} of volume {vid}: {e:#}");
            continue; // row stays `deleting`; retried next run
        }
        sqlx::query("DELETE FROM blobs WHERE volume_id = $1 AND hash = $2")
            .bind(vid)
            .bind(&h)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        removed += 1;
    }
    Ok(removed)
}

fn fake_mount(volume_id: i64, connection_id: i64, base: &str) -> Mount {
    Mount {
        id: 0,
        owner_id: 0,
        path: String::new(),
        volume_id,
        connection_id,
        connection_name: String::new(),
        connection_kind: String::new(),
        base_path: base.to_string(),
        mode: "hashed".into(),
        journal_from: 0,
        created_at: chrono::Utc::now(),
        range_read: true,
    }
}

/// Remove files older than `age` below `dir`.
async fn clean_dir(dir: &std::path::Path, age: Duration) {
    let dir = dir.to_path_buf();
    let _ = tokio::task::spawn_blocking(move || {
        fn walk(p: &std::path::Path, age: Duration) {
            let Ok(rd) = std::fs::read_dir(p) else { return };
            for e in rd.flatten() {
                let path = e.path();
                if path.is_dir() {
                    walk(&path, age);
                    let _ = std::fs::remove_dir(&path); // only succeeds when empty
                } else if let Ok(md) = e.metadata()
                    && md
                        .modified()
                        .ok()
                        .and_then(|m| m.elapsed().ok())
                        .is_some_and(|el| el > age)
                {
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
        walk(&dir, age)
    })
    .await;
}
