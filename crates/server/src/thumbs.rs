//! Image thumbnails, cached on local disk by content identity.

use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::http::header::{self, HeaderMap, HeaderValue};
use axum::response::{IntoResponse, Response};
use futures::StreamExt;
use serde::Deserialize;
use strata_storage::paths;

use crate::auth::AuthUser;
use crate::error::{ApiError, ApiResult, bad};
use crate::mounts::{self, Mount};
use crate::state::AppState;
use crate::vfs::{self, Node};

const MAX_SOURCE: i64 = 64 * 1024 * 1024;

#[derive(Deserialize)]
pub struct ThumbQuery {
    pub path: String,
    pub size: Option<u32>,
}

pub fn is_thumbable(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    [
        ".jpg", ".jpeg", ".png", ".gif", ".webp", ".bmp", ".tif", ".tiff",
    ]
    .iter()
    .any(|e| n.ends_with(e))
}

pub async fn thumbnail(
    st: &AppState,
    m: &Mount,
    rel: &str,
    node: &Node,
    size: u32,
    headers: &HeaderMap,
) -> ApiResult<Response> {
    if node.is_dir || !is_thumbable(&node.name) {
        return Err(bad("no thumbnail for this file"));
    }
    if node.size > MAX_SOURCE {
        return Err(bad("image too large for a thumbnail"));
    }
    let size = size.clamp(32, 1024);
    let key = blake3::hash(
        format!(
            "{}|{}|{}|{}|{}",
            m.volume_id,
            rel,
            node.etag.as_deref().unwrap_or(""),
            node.size,
            size
        )
        .as_bytes(),
    )
    .to_hex()
    .to_string();
    let etag = format!("\"t{}\"", &key[..24]);
    if headers
        .get(header::IF_NONE_MATCH)
        .is_some_and(|v| v.as_bytes() == etag.as_bytes())
    {
        let mut resp = StatusCode::NOT_MODIFIED.into_response();
        crate::safe::protect(resp.headers_mut());
        return Ok(resp);
    }
    let dir = st.cfg.data_dir.join("thumbs").join(&key[..2]);
    let file = dir.join(format!("{key}.jpg"));
    let bytes = match tokio::fs::read(&file).await {
        Ok(b) => b,
        Err(_) => {
            let mut s = vfs::read_stream(st, m, rel, node, 0, node.size as u64).await?;
            let mut src = Vec::with_capacity(node.size as usize);
            while let Some(b) = s.next().await {
                src.extend_from_slice(&b.map_err(|e| ApiError::Internal(e.into()))?);
            }
            let out = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<u8>> {
                let img = image::load_from_memory(&src)?;
                let t = img.thumbnail(size, size).to_rgb8();
                let mut out = Vec::new();
                image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 80)
                    .encode_image(&t)?;
                Ok(out)
            })
            .await
            .map_err(|e| ApiError::Internal(e.into()))?
            .map_err(|e| bad(format!("cannot decode image: {e}")))?;
            tokio::fs::create_dir_all(&dir).await.ok();
            let tmp = file.with_extension(format!("tmp{}", rand::random::<u32>()));
            if tokio::fs::write(&tmp, &out).await.is_ok() {
                let _ = tokio::fs::rename(&tmp, &file).await;
            }
            out
        }
    };
    let mut resp = Response::new(Body::from(bytes));
    let h = resp.headers_mut();
    crate::safe::protect(h);
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("image/jpeg"));
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=86400"),
    );
    h.insert(header::ETAG, HeaderValue::from_str(&etag).unwrap());
    Ok(resp)
}

pub async fn get(
    State(st): State<AppState>,
    AuthUser(u): AuthUser,
    Query(q): Query<ThumbQuery>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let path = paths::normalize(&q.path)?;
    let (m, rel) = mounts::resolve(&st, u.id, &path).await?;
    let node = vfs::stat(&st, &m, &rel).await?;
    thumbnail(&st, &m, &rel, &node, q.size.unwrap_or(256), &headers).await
}
