use axum::extract::DefaultBodyLimit;
use axum::http::{HeaderValue, header};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde_json::json;
use tower_http::compression::{CompressionLayer, Predicate};
use tower_http::services::{ServeDir, ServeFile};
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::trace::TraceLayer;

use crate::state::AppState;
use crate::{auth, connections, files, journal, mounts, shares, thumbs, uploads};

/// Liveness and readiness: 200 only when the database answers.
async fn healthz(
    axum::extract::State(st): axum::extract::State<AppState>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    match sqlx::query("SELECT 1").execute(&st.db).await {
        Ok(_) => Json(json!({ "ok": true, "version": env!("CARGO_PKG_VERSION") })).into_response(),
        Err(e) => {
            tracing::warn!("health check: database unavailable: {e}");
            (
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "ok": false })),
            )
                .into_response()
        }
    }
}

pub fn router(st: AppState) -> Router {
    let api = Router::new()
        .route("/health", get(healthz))
        .route("/auth/login", post(auth::login))
        .route("/auth/logout", post(auth::logout))
        .route("/auth/me", get(auth::me))
        .route("/auth/password", post(auth::change_password))
        .route(
            "/admin/users",
            get(auth::list_users).post(auth::admin_create_user),
        )
        .route(
            "/admin/users/{id}",
            axum::routing::delete(auth::admin_delete_user),
        )
        .route("/admin/users/{id}/password", post(auth::admin_set_password))
        .route(
            "/connections",
            get(connections::list).post(connections::create),
        )
        .route("/connections/test", post(connections::test))
        .route(
            "/connections/{id}",
            get(connections::get)
                .put(connections::update)
                .delete(connections::delete),
        )
        .route("/connections/{id}/probe", post(connections::probe))
        .route("/mounts", get(mounts::list).post(mounts::create))
        .route("/mounts/{id}", axum::routing::delete(mounts::delete))
        .route("/mounts/{id}/scan", post(mounts::scan))
        .route("/files/list", get(files::list))
        .route("/files/stat", get(files::stat))
        .route("/files/content", get(files::content))
        .route("/files/thumb", get(thumbs::get))
        .route("/files/image", get(files::image))
        .route("/files/mkdir", post(files::mkdir))
        .route("/files/move", post(files::move_))
        .route("/files/copy", post(files::copy))
        .route("/files/delete", post(files::delete))
        .route("/search", get(files::search))
        .route("/uploads", post(uploads::create))
        .route(
            "/uploads/{id}",
            get(uploads::status).delete(uploads::cancel),
        )
        .route(
            "/uploads/{id}/chunks/{hash}",
            put(uploads::put_chunk).layer(DefaultBodyLimit::max(
                strata_storage::chunker::MAX_SIZE + 4096,
            )),
        )
        .route("/uploads/{id}/commit", post(uploads::commit))
        .route("/changes", get(journal::changes))
        .route("/ws", get(journal::ws))
        .route("/shares", get(shares::list).post(shares::create))
        .route("/shares/{id}", axum::routing::delete(shares::delete))
        .route("/public/{token}", get(shares::info))
        .route("/public/{token}/unlock", post(shares::unlock))
        .route("/public/{token}/content", get(shares::content))
        .route("/public/{token}/thumb", get(shares::thumb))
        .route("/public/{token}/image", get(shares::image))
        .fallback(|| async { crate::error::ApiError::NotFound })
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        ));

    let web = &st.cfg.web_dir;
    let index = web.join("index.html");
    let static_files = ServeDir::new(web)
        .precompressed_gzip()
        .fallback(ServeFile::new(&index));

    Router::new()
        .route(
            "/healthz",
            get(healthz).layer(SetResponseHeaderLayer::overriding(
                header::CACHE_CONTROL,
                HeaderValue::from_static("no-store"),
            )),
        )
        .nest("/api", api)
        .fallback_service(static_files)
        .layer(
            CompressionLayer::new().gzip(true).compress_when(
                tower_http::compression::predicate::SizeAbove::new(1024)
                    .and(tower_http::compression::predicate::NotForContentType::new(
                        "image/",
                    ))
                    .and(tower_http::compression::predicate::NotForContentType::new(
                        "video/",
                    ))
                    .and(tower_http::compression::predicate::NotForContentType::new(
                        "application/octet-stream",
                    )),
            ),
        )
        .layer(SetResponseHeaderLayer::if_not_present(
            header::HeaderName::from_static("x-frame-options"),
            HeaderValue::from_static("DENY"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::HeaderName::from_static("referrer-policy"),
            HeaderValue::from_static("same-origin"),
        ))
        .layer(TraceLayer::new_for_http())
        .with_state(st)
}
