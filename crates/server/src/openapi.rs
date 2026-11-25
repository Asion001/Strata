//! The HTTP API as an OpenAPI 3.1 document, generated from the code
//! (utoipa) and served at `/api/v1/openapi.json`. A snapshot is committed at
//! `api/openapi.json`; a test fails when the two differ (run it with
//! `UPDATE_OPENAPI=1` to rewrite the snapshot), and the web client's types
//! are generated from that snapshot.
//!
//! Request bodies use the handlers' own types. Responses that handlers build
//! with `json!` are described by the `*Body` types here; Schemathesis checks
//! every operation's responses against them in CI.

#![allow(dead_code)] // (the stubs below only carry the documentation)

use chrono::{DateTime, Utc};
use serde::Serialize;
use utoipa::openapi::link::LinkBuilder;
use utoipa::openapi::path::Operation;
use utoipa::openapi::security::{
    ApiKey, ApiKeyValue, HttpAuthScheme, HttpBuilder, SecurityRequirement, SecurityScheme,
};
use utoipa::openapi::{ContentBuilder, OpenApi as Doc, RefOr, ResponseBuilder};
use utoipa::{Modify, OpenApi, ToSchema};

use crate::auth::{ChangePasswordReq, CreateUserReq, LoginReq, SetPasswordReq, User};
use crate::connections::{ConnectionView, TestReq, UpsertReq};
use crate::files::{DeleteBody, FromTo, PathBody};
use crate::journal::FeedEntry;
use crate::mounts::{CreateReq as MountInput, Mount};
use crate::shares::{CreateReq as ShareInput, Share, UnlockReq};
use crate::uploads::{ChunkSpec, CreateReq as UploadInput};
use crate::vfs::{Entry, SearchHit};

// ---------------------------------------------------------------- schemas

/// Every error: a message for people (never internal details).
#[derive(Serialize, ToSchema)]
pub struct ErrorBody {
    pub error: String,
    /// On 412: the file's current version, if it exists.
    #[schema(nullable)]
    pub etag: Option<String>,
}

/// An answer with nothing to say.
#[derive(Serialize, ToSchema)]
pub struct EmptyBody {}

/// Bytes of a file, as stored.
#[derive(ToSchema)]
#[schema(value_type = String, format = Binary)]
pub struct Binary(Vec<u8>);

#[derive(Serialize, ToSchema)]
pub struct HealthBody {
    pub ok: bool,
    pub version: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct LoginBody {
    pub user: User,
    /// A session token for `Authorization: Bearer` (the response also sets
    /// the session cookie).
    pub token: String,
}

#[derive(Serialize, ToSchema)]
pub struct ConnectionTestBody {
    pub ok: bool,
    /// The backend's message: admins only.
    #[schema(nullable)]
    pub error: Option<String>,
    /// Why it failed, in plain words.
    #[schema(nullable)]
    pub reason: Option<String>,
    pub note: Option<String>,
}

/// A new mount, and other mounts showing an overlapping folder.
#[derive(Serialize, ToSchema)]
pub struct MountCreatedBody {
    #[serde(flatten)]
    pub mount: Mount,
    pub overlaps: Vec<String>,
}

#[derive(Serialize, ToSchema)]
pub struct ScanBody {
    pub started: bool,
    pub reason: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct ListingMount {
    pub id: i64,
    pub path: String,
    pub mode: String,
    pub connection_name: String,
    pub connection_kind: String,
    pub offline: bool,
}

#[derive(Serialize, ToSchema)]
pub struct ListingBody {
    pub path: String,
    /// Whose listing this is.
    pub user: i64,
    pub mount: ListingMount,
    pub entries: Vec<Entry>,
}

#[derive(Serialize, ToSchema)]
pub struct StatBody {
    pub path: String,
    pub name: String,
    pub is_dir: bool,
    pub size: i64,
    pub mtime: DateTime<Utc>,
    #[schema(nullable)]
    pub etag: Option<String>,
    pub mode: String,
}

#[derive(Serialize, ToSchema)]
pub struct PathResultBody {
    pub path: String,
}

#[derive(Serialize, ToSchema)]
pub struct DeletedBody {
    pub deleted: i64,
}

#[derive(Serialize, ToSchema)]
pub struct SearchBody {
    pub hits: Vec<SearchHit>,
}

#[derive(Serialize, ToSchema)]
pub struct SavedBody {
    pub path: String,
    pub size: i64,
    pub mtime: DateTime<Utc>,
    pub etag: String,
}

#[derive(Serialize, ToSchema)]
pub struct UploadCreatedBody {
    pub id: String,
    /// Indexes of the chunks the server still needs.
    pub missing: Vec<i64>,
}

#[derive(Serialize, ToSchema)]
pub struct UploadStatusBody {
    pub id: String,
    pub path: String,
    pub size: i64,
    pub missing: Vec<i64>,
}

#[derive(Serialize, ToSchema)]
pub struct CommittedBody {
    pub path: String,
    pub name: String,
    pub size: i64,
    pub mtime: DateTime<Utc>,
    #[schema(nullable)]
    pub etag: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct ChangesBody {
    /// Pass back as `cursor` to get what changed since.
    pub cursor: String,
    pub more: bool,
    pub changes: Vec<FeedEntry>,
}

#[derive(Serialize, ToSchema)]
pub struct ResyncBody {
    pub error: String,
    /// The cursor is too old: list again from scratch.
    pub resync: bool,
}

/// A public share: locked (`needs_password`), a folder (with `entries`) or
/// a file (with `size` and `mtime`).
#[derive(Serialize, ToSchema)]
pub struct ShareInfoBody {
    pub name: String,
    pub is_dir: bool,
    pub needs_password: bool,
    pub path: Option<String>,
    pub entries: Option<Vec<Entry>>,
    pub size: Option<i64>,
    pub mtime: Option<DateTime<Utc>>,
}

#[derive(Serialize, ToSchema)]
pub struct UnlockBody {
    /// Pass as `k` to the other public routes (null: no password needed).
    #[schema(nullable)]
    pub key: Option<String>,
}

// ---------------------------------------------------------------- operations
// Stubs carrying each route's documentation; the handlers are in their
// modules. Shared error answers (400, 401, 403, 404, 415, 422, 500) are
// added to every operation by `Defaults` below.

macro_rules! stub {
    ($($name:ident),* $(,)?) => { $( fn $name() {} )* };
}

#[utoipa::path(get, path = "/healthz", tag = "health", responses(
    (status = 200, body = HealthBody), (status = 503, description = "The database doesn't answer", body = HealthBody)))]
fn healthz() {}
#[utoipa::path(get, path = "/api/health", tag = "health", responses(
    (status = 200, body = HealthBody), (status = 503, description = "The database doesn't answer", body = HealthBody)))]
fn health() {}
#[utoipa::path(get, path = "/api/v1/openapi.json", tag = "health", responses(
    (status = 200, description = "This document", content_type = "application/json", body = Object)))]
fn openapi_json() {}

#[utoipa::path(post, path = "/api/auth/login", tag = "auth", request_body = LoginReq, responses(
    (status = 200, description = "Signed in; sets the session cookie", body = LoginBody)))]
fn login() {}
#[utoipa::path(post, path = "/api/auth/logout", tag = "auth", responses(
    (status = 200, description = "Signed out (the session ends, the cookie is cleared)", body = EmptyBody)))]
fn logout() {}
#[utoipa::path(get, path = "/api/auth/me", tag = "auth", responses((status = 200, body = User)))]
fn me() {}
#[utoipa::path(post, path = "/api/auth/password", tag = "auth", request_body = ChangePasswordReq,
    responses((status = 200, body = EmptyBody)))]
fn change_password() {}

#[utoipa::path(get, path = "/api/admin/users", tag = "admin", responses((status = 200, body = Vec<User>)))]
fn list_users() {}
#[utoipa::path(post, path = "/api/admin/users", tag = "admin", request_body = CreateUserReq, responses(
    (status = 200, body = User), (status = 409, description = "That name is taken", body = ErrorBody)))]
fn create_user() {}
#[utoipa::path(delete, path = "/api/admin/users/{id}", tag = "admin", params(("id" = i64, Path)), responses(
    (status = 200, body = EmptyBody)))]
fn delete_user() {}
#[utoipa::path(post, path = "/api/admin/users/{id}/password", tag = "admin", params(("id" = i64, Path)),
    request_body = SetPasswordReq, responses((status = 200, body = EmptyBody)))]
fn set_user_password() {}

#[utoipa::path(get, path = "/api/connections", tag = "connections", responses((status = 200, body = Vec<ConnectionView>)))]
fn list_connections() {}
#[utoipa::path(post, path = "/api/connections", tag = "connections", request_body = UpsertReq, responses(
    (status = 200, body = ConnectionView), (status = 409, body = ErrorBody)))]
fn create_connection() {}
#[utoipa::path(post, path = "/api/connections/test", tag = "connections", request_body = TestReq, responses(
    (status = 200, description = "Whether the storage can be reached", body = ConnectionTestBody)))]
fn test_connection() {}
#[utoipa::path(get, path = "/api/connections/{id}", tag = "connections", params(("id" = i64, Path)), responses(
    (status = 200, body = ConnectionView)))]
fn get_connection() {}
#[utoipa::path(put, path = "/api/connections/{id}", tag = "connections", params(("id" = i64, Path)),
    request_body = UpsertReq, responses((status = 200, body = ConnectionView), (status = 409, body = ErrorBody)))]
fn update_connection() {}
#[utoipa::path(delete, path = "/api/connections/{id}", tag = "connections", params(("id" = i64, Path)), responses(
    (status = 200, body = EmptyBody), (status = 409, description = "Still in use", body = ErrorBody)))]
fn delete_connection() {}
#[utoipa::path(post, path = "/api/connections/{id}/probe", tag = "connections", params(("id" = i64, Path)), responses(
    (status = 200, body = ConnectionView)))]
fn probe_connection() {}

#[utoipa::path(get, path = "/api/mounts", tag = "mounts", responses((status = 200, body = Vec<Mount>)))]
fn list_mounts() {}
#[utoipa::path(post, path = "/api/mounts", tag = "mounts", request_body = MountInput, responses(
    (status = 200, body = MountCreatedBody), (status = 409, body = ErrorBody), (status = 503, body = ErrorBody)))]
fn create_mount() {}
#[utoipa::path(delete, path = "/api/mounts/{id}", tag = "mounts", params(("id" = i64, Path)), responses(
    (status = 200, body = EmptyBody), (status = 409, body = ErrorBody)))]
fn delete_mount() {}
#[utoipa::path(post, path = "/api/mounts/{id}/scan", tag = "mounts", params(("id" = i64, Path)), responses(
    (status = 200, body = ScanBody), (status = 503, body = ErrorBody)))]
fn scan_mount() {}

#[utoipa::path(get, path = "/api/files/list", tag = "files",
    params(("path" = Option<String>, Query, description = "Folder ('' = home)")),
    responses((status = 200, body = ListingBody), (status = 503, description = "The storage is offline", body = ErrorBody)))]
fn list_files() {}
#[utoipa::path(get, path = "/api/files/stat", tag = "files", params(("path" = Option<String>, Query)), responses(
    (status = 200, body = StatBody), (status = 503, body = ErrorBody)))]
fn stat_file() {}
#[utoipa::path(get, path = "/api/files/content", tag = "files",
    params(("path" = Option<String>, Query), ("download" = Option<String>, Query, description = "1: as an attachment"),
        ("Range" = Option<String>, Header, description = "One byte range (bytes=a-b, a-, -n)"),
        ("If-Range" = Option<String>, Header), ("If-None-Match" = Option<String>, Header)),
    responses(
        (status = 200, description = "The file", content_type = "application/octet-stream", body = Binary),
        (status = 206, description = "A byte range", content_type = "application/octet-stream", body = Binary),
        (status = 304, description = "Not modified"),
        (status = 416, description = "Range not satisfiable"),
        (status = 503, body = ErrorBody)))]
fn get_content() {}
#[utoipa::path(put, path = "/api/files/content", tag = "files",
    params(("path" = Option<String>, Query),
        ("If-Match" = Option<String>, Header, description = "The version being replaced"),
        ("If-None-Match" = Option<String>, Header, description = "*: create only"),
        ("X-Strata-Base-Hash" = Option<String>, Header, description = "BLAKE3 of the bytes being replaced (raw folders)")),
    request_body(content = Binary, content_type = "application/octet-stream"),
    responses(
        (status = 200, body = SavedBody),
        (status = 409, body = ErrorBody),
        (status = 412, description = "Changed by someone else", body = ErrorBody),
        (status = 413, description = "Too large to edit here"),
        (status = 428, description = "A precondition header is missing", body = ErrorBody),
        (status = 503, body = ErrorBody)))]
fn put_content() {}
#[utoipa::path(get, path = "/api/files/thumb", tag = "files",
    params(("path" = String, Query), ("size" = Option<u32>, Query), ("v" = Option<String>, Query, description = "Cache key")),
    responses((status = 200, content_type = "image/jpeg", body = Binary), (status = 304, description = "Not modified"),
        (status = 503, body = ErrorBody)))]
fn thumbnail() {}
#[utoipa::path(get, path = "/api/files/image", tag = "files", params(("path" = Option<String>, Query)), responses(
    (status = 200, description = "The file, as an image (SVG too)", content_type = "image/*", body = Binary),
    (status = 304, description = "Not modified"), (status = 503, body = ErrorBody)))]
fn get_image() {}
#[utoipa::path(post, path = "/api/files/mkdir", tag = "files", request_body = PathBody, responses(
    (status = 200, body = PathResultBody), (status = 409, body = ErrorBody), (status = 503, body = ErrorBody)))]
fn mkdir() {}
#[utoipa::path(post, path = "/api/files/move", tag = "files", request_body = FromTo, responses(
    (status = 200, body = PathResultBody), (status = 409, body = ErrorBody), (status = 503, body = ErrorBody)))]
fn move_file() {}
#[utoipa::path(post, path = "/api/files/copy", tag = "files", request_body = FromTo, responses(
    (status = 200, body = PathResultBody), (status = 409, body = ErrorBody), (status = 503, body = ErrorBody)))]
fn copy_file() {}
#[utoipa::path(post, path = "/api/files/delete", tag = "files", request_body = DeleteBody, responses(
    (status = 200, body = DeletedBody), (status = 503, body = ErrorBody)))]
fn delete_files() {}
#[utoipa::path(get, path = "/api/search", tag = "files",
    params(("q" = String, Query), ("limit" = Option<i64>, Query)), responses((status = 200, body = SearchBody)))]
fn search() {}

#[utoipa::path(post, path = "/api/uploads", tag = "uploads", request_body = UploadInput, responses(
    (status = 200, body = UploadCreatedBody), (status = 409, body = ErrorBody), (status = 503, body = ErrorBody)))]
fn create_upload() {}
#[utoipa::path(get, path = "/api/uploads/{id}", tag = "uploads", params(("id" = String, Path)), responses(
    (status = 200, body = UploadStatusBody), (status = 503, body = ErrorBody)))]
fn upload_status() {}
#[utoipa::path(delete, path = "/api/uploads/{id}", tag = "uploads", params(("id" = String, Path)), responses(
    (status = 200, body = EmptyBody)))]
fn cancel_upload() {}
#[utoipa::path(put, path = "/api/uploads/{id}/chunks/{hash}", tag = "uploads",
    params(("id" = String, Path), ("hash" = String, Path, description = "BLAKE3 of the chunk (hex)")),
    request_body(content = Binary, content_type = "application/octet-stream"),
    responses((status = 200, body = EmptyBody), (status = 413, description = "Larger than a chunk can be"),
        (status = 503, body = ErrorBody)))]
fn put_chunk() {}
#[utoipa::path(post, path = "/api/uploads/{id}/commit", tag = "uploads", params(("id" = String, Path)), responses(
    (status = 200, body = CommittedBody), (status = 409, description = "Chunks missing, or the folder changed", body = ErrorBody),
    (status = 503, body = ErrorBody)))]
fn commit_upload() {}

#[utoipa::path(get, path = "/api/changes", tag = "changes",
    params(("cursor" = Option<String>, Query), ("limit" = Option<i64>, Query)),
    responses((status = 200, body = ChangesBody), (status = 410, description = "Cursor too old: resync", body = ResyncBody)))]
fn changes() {}
#[utoipa::path(get, path = "/api/ws", tag = "changes", responses(
    (status = 101, description = "WebSocket: change notifications (cursor, health)")))]
fn websocket() {}

#[utoipa::path(get, path = "/api/shares", tag = "shares", responses((status = 200, body = Vec<Share>)))]
fn list_shares() {}
#[utoipa::path(post, path = "/api/shares", tag = "shares", request_body = ShareInput, responses(
    (status = 200, body = Share), (status = 503, body = ErrorBody)))]
fn create_share() {}
#[utoipa::path(delete, path = "/api/shares/{id}", tag = "shares", params(("id" = i64, Path)), responses(
    (status = 200, body = EmptyBody)))]
fn delete_share() {}

#[utoipa::path(get, path = "/api/public/{token}", tag = "public",
    params(("token" = String, Path), ("path" = Option<String>, Query), ("k" = Option<String>, Query)),
    responses((status = 200, body = ShareInfoBody), (status = 503, body = ErrorBody)))]
fn share_info() {}
#[utoipa::path(post, path = "/api/public/{token}/unlock", tag = "public", params(("token" = String, Path)),
    request_body = UnlockReq, responses((status = 200, body = UnlockBody)))]
fn unlock_share() {}
#[utoipa::path(get, path = "/api/public/{token}/content", tag = "public",
    params(("token" = String, Path), ("path" = Option<String>, Query), ("k" = Option<String>, Query),
        ("download" = Option<String>, Query), ("Range" = Option<String>, Header)),
    responses((status = 401, description = "A password-protected share without its key", body = ErrorBody), (status = 200, content_type = "application/octet-stream", body = Binary),
        (status = 206, content_type = "application/octet-stream", body = Binary),
        (status = 304, description = "Not modified"), (status = 416, description = "Range not satisfiable"),
        (status = 503, body = ErrorBody)))]
fn share_content() {}
#[utoipa::path(get, path = "/api/public/{token}/thumb", tag = "public",
    params(("token" = String, Path), ("path" = Option<String>, Query), ("k" = Option<String>, Query), ("size" = Option<u32>, Query)),
    responses((status = 401, description = "A password-protected share without its key", body = ErrorBody), (status = 200, content_type = "image/jpeg", body = Binary), (status = 304, description = "Not modified"),
        (status = 503, body = ErrorBody)))]
fn share_thumb() {}
#[utoipa::path(get, path = "/api/public/{token}/image", tag = "public",
    params(("token" = String, Path), ("path" = Option<String>, Query), ("k" = Option<String>, Query)),
    responses((status = 401, description = "A password-protected share without its key", body = ErrorBody), (status = 200, content_type = "image/*", body = Binary), (status = 304, description = "Not modified"),
        (status = 503, body = ErrorBody)))]
fn share_image() {}

stub!();

/// Operations anyone may call (no session).
const PUBLIC: &[&str] = &[
    "healthz",
    "health",
    "openapi_json",
    "login",
    "logout",
    "share_info",
    "unlock_share",
    "share_content",
    "share_thumb",
    "share_image",
];

/// Shared answers, security and links, added to every operation.
struct Defaults;

impl Modify for Defaults {
    fn modify(&self, doc: &mut Doc) {
        let comps = doc.components.get_or_insert_with(Default::default);
        comps.add_security_scheme(
            "bearer",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .description(Some("The token from /api/auth/login"))
                    .build(),
            ),
        );
        comps.add_security_scheme(
            "session",
            SecurityScheme::ApiKey(ApiKey::Cookie(ApiKeyValue::with_description(
                "strata_session",
                "The session cookie. Requests other than GET/HEAD must also send an `X-Strata` header (any value): browsers can't add it cross-site.",
            ))),
        );
        let error = |desc: &str| {
            ResponseBuilder::new()
                .description(desc)
                .content(
                    "application/json",
                    ContentBuilder::new()
                        .schema(Some(RefOr::Ref(utoipa::openapi::Ref::from_schema_name(
                            "ErrorBody",
                        ))))
                        .build(),
                )
                .build()
        };
        for (path, item) in doc.paths.paths.iter_mut() {
            for (method, op) in [
                ("get", item.get.as_mut()),
                ("put", item.put.as_mut()),
                ("post", item.post.as_mut()),
                ("delete", item.delete.as_mut()),
            ] {
                let Some(op) = op else { continue };
                let id = op.operation_id.clone().unwrap_or_default();
                let public = PUBLIC.contains(&id.as_str());
                if public {
                    op.security = Some(vec![]);
                } else {
                    op.security = Some(vec![
                        SecurityRequirement::new("bearer", Vec::<String>::new()),
                        SecurityRequirement::new("session", Vec::<String>::new()),
                    ]);
                }
                let has_body = op.request_body.is_some();
                let r = &mut op.responses.responses;
                let mut add = |code: &str, desc: &str| {
                    r.entry(code.to_string())
                        .or_insert_with(|| RefOr::T(error(desc)));
                };
                add("400", "Bad request (a parameter or the body)");
                if !public {
                    add("401", "Not signed in");
                    add(
                        "403",
                        "Not allowed (or a cookie request without X-Strata, or a read-only folder)",
                    );
                }
                add("404", "Not found");
                if has_body {
                    add("413", "The body is too large");
                    add("415", "The body has the wrong content type");
                    add("422", "The body doesn't match the schema");
                }
                add("500", "Internal error (details are only logged)");
                let _ = (path, method);
                links(&id, op);
            }
        }
    }
}

/// Create, then get, then delete: what an id from a create can be used for.
fn links(id: &str, op: &mut Operation) {
    let follow: &[(&str, &[&str])] = &[
        ("create_user", &["delete_user", "set_user_password"]),
        (
            "create_connection",
            &[
                "get_connection",
                "update_connection",
                "probe_connection",
                "delete_connection",
            ],
        ),
        ("create_mount", &["scan_mount", "delete_mount"]),
        (
            "create_upload",
            &["upload_status", "commit_upload", "cancel_upload"],
        ),
        ("create_share", &["delete_share"]),
    ];
    let Some((_, targets)) = follow.iter().find(|(f, _)| *f == id) else {
        return;
    };
    let Some(RefOr::T(resp)) = op.responses.responses.get_mut("200") else {
        return;
    };
    for t in *targets {
        resp.links.insert(
            t.to_string(),
            RefOr::T(
                LinkBuilder::new()
                    .operation_id(*t)
                    .parameter("id", "$response.body#/id")
                    .build(),
            ),
        );
    }
}

#[derive(OpenApi)]
#[openapi(
    info(title = "Strata API", description = "The HTTP API of Strata, a self-hosted file cloud.", license(name = "AGPL-3.0-or-later")),
    paths(
        healthz, health, openapi_json,
        login, logout, me, change_password,
        list_users, create_user, delete_user, set_user_password,
        list_connections, create_connection, test_connection, get_connection, update_connection, delete_connection, probe_connection,
        list_mounts, create_mount, delete_mount, scan_mount,
        list_files, stat_file, get_content, put_content, thumbnail, get_image, mkdir, move_file, copy_file, delete_files, search,
        create_upload, upload_status, cancel_upload, put_chunk, commit_upload,
        changes, websocket,
        list_shares, create_share, delete_share,
        share_info, unlock_share, share_content, share_thumb, share_image,
    ),
    components(schemas(ErrorBody, ChunkSpec)),
    modifiers(&Defaults),
    tags(
        (name = "health"), (name = "auth"), (name = "admin", description = "Administrators only"),
        (name = "connections", description = "Storage connections"), (name = "mounts"), (name = "files"),
        (name = "uploads", description = "Resumable, chunked uploads"), (name = "changes"), (name = "shares"),
        (name = "public", description = "Share links (no session)"),
    )
)]
pub struct ApiDoc;

/// The document as served and as committed (stable, pretty JSON).
pub fn json() -> String {
    let mut v = serde_json::to_value(ApiDoc::openapi()).expect("the spec serializes");
    nullable_is_required(&mut v);
    camel_case_links(&mut v);
    serde_json::to_string_pretty(&v).expect("the spec serializes") + "\n"
}

/// Answers whose optional keys are left out, not sent as null.
const SPARSE: &[&str] = &[
    "ShareInfoBody",
    "ConnectionTestBody",
    "ScanBody",
    "HealthBody",
];

/// utoipa 5 writes link fields in snake_case (`operation_id`); OpenAPI
/// says camelCase.
fn camel_case_links(v: &mut serde_json::Value) {
    for item in v["paths"]
        .as_object_mut()
        .into_iter()
        .flat_map(|p| p.values_mut())
    {
        for op in item
            .as_object_mut()
            .into_iter()
            .flat_map(|o| o.values_mut())
        {
            for resp in op["responses"]
                .as_object_mut()
                .into_iter()
                .flat_map(|r| r.values_mut())
            {
                for link in resp
                    .get_mut("links")
                    .and_then(|l| l.as_object_mut())
                    .into_iter()
                    .flat_map(|l| l.values_mut())
                {
                    let Some(l) = link.as_object_mut() else {
                        continue;
                    };
                    for (from, to) in [
                        ("operation_id", "operationId"),
                        ("operation_ref", "operationRef"),
                        ("request_body", "requestBody"),
                    ] {
                        if let Some(x) = l.remove(from) {
                            l.insert(to.to_string(), x);
                        }
                    }
                }
            }
        }
    }
}

/// In what the server sends, an `Option` field is always there (as null
/// when empty), so in response schemas it is required (and nullable). What
/// clients send (request bodies, and the schemas they use) stays optional.
fn nullable_is_required(v: &mut serde_json::Value) {
    use serde_json::Value;
    fn refs(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::Object(m) => {
                if let Some(Value::String(r)) = m.get("$ref") {
                    out.push(r.trim_start_matches("#/components/schemas/").to_string());
                }
                m.values().for_each(|x| refs(x, out));
            }
            Value::Array(a) => a.iter().for_each(|x| refs(x, out)),
            _ => {}
        }
    }
    // Schemas reachable from request bodies.
    let mut input = Vec::new();
    for item in v["paths"].as_object().into_iter().flat_map(|p| p.values()) {
        for op in item.as_object().into_iter().flat_map(|o| o.values()) {
            refs(&op["requestBody"], &mut input);
        }
    }
    let mut i = 0;
    while i < input.len() {
        let mut more = Vec::new();
        refs(&v["components"]["schemas"][&input[i]], &mut more);
        for m in more {
            if !input.contains(&m) {
                input.push(m);
            }
        }
        i += 1;
    }
    let Some(schemas) = v["components"]["schemas"].as_object_mut() else {
        return;
    };
    for (name, schema) in schemas.iter_mut() {
        if input.contains(name) {
            continue;
        }
        let Some(props) = schema
            .get("properties")
            .and_then(|p| p.as_object())
            .cloned()
        else {
            continue;
        };
        let nullable = |p: &Value| {
            p["type"]
                .as_array()
                .is_some_and(|t| t.iter().any(|x| x == "null"))
                || p["oneOf"]
                    .as_array()
                    .is_some_and(|o| o.iter().any(|x| x["type"] == "null"))
        };
        let required = schema
            .as_object_mut()
            .unwrap()
            .entry("required")
            .or_insert_with(|| Value::Array(vec![]));
        for (k, p) in props {
            // (Left out when there is nothing to say: Capabilities.fs_type
            // when unknown, ErrorBody.etag but on a 412.)
            let omitted = (name == "Capabilities" && k == "fs_type")
                || (name == "ErrorBody" && k == "etag")
                || SPARSE.contains(&name.as_str());
            if nullable(&p) && !omitted && !required.as_array().unwrap().iter().any(|r| r == &k) {
                required.as_array_mut().unwrap().push(Value::String(k));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SNAPSHOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../api/openapi.json");

    /// The committed snapshot is the generated spec (rewrite it with
    /// `UPDATE_OPENAPI=1 cargo test -p strata-server openapi`).
    #[test]
    fn snapshot_is_current() {
        let now = json();
        if std::env::var_os("UPDATE_OPENAPI").is_some() {
            std::fs::write(SNAPSHOT, &now).unwrap();
        }
        let saved = std::fs::read_to_string(SNAPSHOT).unwrap_or_default();
        assert!(
            saved == now,
            "api/openapi.json is out of date: run UPDATE_OPENAPI=1 cargo test -p strata-server openapi"
        );
    }

    /// Every route the router serves is documented, and nothing else is.
    #[test]
    fn every_route_is_documented() {
        let src = include_str!("routes.rs");
        let mut routes: Vec<String> = Vec::new();
        let mut rest = src;
        while let Some(i) = rest.find(".route(") {
            rest = &rest[i + 7..];
            let start = rest.find('"').unwrap() + 1;
            let end = start + rest[start..].find('"').unwrap();
            let p = &rest[start..end];
            routes.push(if p == "/healthz" || p.starts_with("/api/") {
                p.to_string()
            } else {
                format!("/api{p}")
            });
        }
        routes.sort();
        routes.dedup();
        let doc = ApiDoc::openapi();
        let mut documented: Vec<String> = doc.paths.paths.keys().cloned().collect();
        documented.sort();
        assert_eq!(routes, documented);
    }

    /// Every operation documents at least one 4xx answer (with a schema).
    #[test]
    fn every_operation_documents_a_client_error() {
        let doc = ApiDoc::openapi();
        let mut missing = Vec::new();
        for (path, item) in &doc.paths.paths {
            for (m, op) in [
                ("get", &item.get),
                ("put", &item.put),
                ("post", &item.post),
                ("delete", &item.delete),
            ] {
                let Some(op) = op else { continue };
                if !op.responses.responses.keys().any(|k| k.starts_with('4')) {
                    missing.push(format!("{m} {path}"));
                }
            }
        }
        assert!(missing.is_empty(), "no 4xx documented: {missing:?}");
    }

    /// No example or default in the spec looks like a real secret.
    #[test]
    fn no_secrets_in_examples() {
        let s = json();
        for bad in ["AKIA", "-----BEGIN", "password123", "minioadmin"] {
            assert!(!s.contains(bad), "{bad} in the spec");
        }
    }
}
