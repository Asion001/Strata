//! Password login, sessions and user administration.

use argon2::Argon2;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use axum::Json;
use axum::extract::{FromRequestParts, Path, State};
use axum::http::header::{AUTHORIZATION, COOKIE, SET_COOKIE};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, Method};
use axum::response::{IntoResponse, Response};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::json;
use strata_storage::{Capabilities, ConnectionConfig};

use crate::error::{ApiError, ApiResult, bad};
use crate::state::AppState;

pub const SESSION_COOKIE: &str = "strata_session";

#[derive(Clone, Debug, Serialize, sqlx::FromRow)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub is_admin: bool,
}

pub fn hash_password(pw: &str) -> anyhow::Result<String> {
    let salt = SaltString::generate(&mut argon2::password_hash::rand_core::OsRng);
    Ok(Argon2::default()
        .hash_password(pw.as_bytes(), &salt)
        .map_err(|e| anyhow::anyhow!("hash: {e}"))?
        .to_string())
}

pub fn verify_password(pw: &str, hash: &str) -> bool {
    PasswordHash::new(hash)
        .map(|h| Argon2::default().verify_password(pw.as_bytes(), &h).is_ok())
        .unwrap_or(false)
}

/// Argon2 is deliberately slow; keep it off the async workers.
pub async fn verify_password_async(pw: String, hash: String) -> bool {
    tokio::task::spawn_blocking(move || verify_password(&pw, &hash))
        .await
        .unwrap_or(false)
}

fn token_hash(token: &str) -> Vec<u8> {
    blake3::hash(token.as_bytes()).as_bytes().to_vec()
}

pub fn random_token(bytes: usize) -> String {
    let mut b = vec![0u8; bytes];
    rand::thread_rng().fill_bytes(&mut b);
    hex::encode(b)
}

fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    for v in headers.get_all(COOKIE) {
        let Ok(s) = v.to_str() else { continue };
        for c in cookie::Cookie::split_parse(s).flatten() {
            if c.name() == name {
                return Some(c.value().to_string());
            }
        }
    }
    None
}

/// The signed-in user. Mutating requests authenticated by cookie must carry
/// an `X-Strata` header (a CSRF guard browsers cannot add cross-site).
pub struct AuthUser(pub User);

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let bearer = parts
            .headers
            .get(AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .map(str::to_string);
        let token = match bearer {
            Some(t) => t,
            None => {
                let t =
                    cookie_value(&parts.headers, SESSION_COOKIE).ok_or(ApiError::Unauthorized)?;
                let safe = matches!(parts.method, Method::GET | Method::HEAD | Method::OPTIONS);
                if !safe && !parts.headers.contains_key("x-strata") {
                    return Err(ApiError::Forbidden);
                }
                t
            }
        };
        let user: Option<User> = sqlx::query_as(
            "SELECT u.id, u.username, u.is_admin FROM sessions s JOIN users u ON u.id = s.user_id
             WHERE s.token_hash = $1 AND s.expires_at > now()",
        )
        .bind(token_hash(&token))
        .fetch_optional(&state.db)
        .await?;
        user.map(AuthUser).ok_or(ApiError::Unauthorized)
    }
}

pub struct AdminUser(pub User);

impl FromRequestParts<AppState> for AdminUser {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let AuthUser(u) = AuthUser::from_request_parts(parts, state).await?;
        if !u.is_admin {
            return Err(ApiError::Forbidden);
        }
        Ok(AdminUser(u))
    }
}

#[derive(Deserialize)]
pub struct LoginReq {
    pub username: String,
    pub password: String,
}

pub async fn login(State(st): State<AppState>, Json(req): Json<LoginReq>) -> ApiResult<Response> {
    let row: Option<(i64, String, bool, String)> = sqlx::query_as(
        "SELECT id, username, is_admin, password_hash FROM users WHERE username = $1",
    )
    .bind(req.username.trim())
    .fetch_optional(&st.db)
    .await?;
    // Verify against a dummy hash for unknown users to keep timing similar.
    let (user, hash) = match row {
        Some((id, username, is_admin, h)) => (
            Some(User {
                id,
                username,
                is_admin,
            }),
            h,
        ),
        None => (None, DUMMY_HASH.clone()),
    };
    let ok = verify_password_async(req.password, hash).await;
    let Some(user) = user.filter(|_| ok) else {
        return Err(ApiError::BadRequest("wrong username or password".into()));
    };
    let token = random_token(32);
    sqlx::query(
        "INSERT INTO sessions (token_hash, user_id, expires_at) VALUES ($1, $2, now() + make_interval(days => $3))",
    )
    .bind(token_hash(&token))
    .bind(user.id)
    .bind(st.cfg.session_days as i32)
    .execute(&st.db)
    .await?;
    let cookie = format!(
        "{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}{}",
        st.cfg.session_days * 86400,
        if st.cfg.cookie_secure { "; Secure" } else { "" }
    );
    let mut resp = Json(json!({ "user": user, "token": token })).into_response();
    resp.headers_mut()
        .insert(SET_COOKIE, HeaderValue::from_str(&cookie).unwrap());
    Ok(resp)
}

static DUMMY_HASH: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| hash_password("strata-dummy-password").unwrap());

pub async fn logout(State(st): State<AppState>, headers: HeaderMap) -> ApiResult<Response> {
    if let Some(t) = cookie_value(&headers, SESSION_COOKIE) {
        sqlx::query("DELETE FROM sessions WHERE token_hash = $1")
            .bind(token_hash(&t))
            .execute(&st.db)
            .await?;
    }
    let mut resp = Json(json!({})).into_response();
    resp.headers_mut().insert(
        SET_COOKIE,
        HeaderValue::from_static("strata_session=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0"),
    );
    Ok(resp)
}

pub async fn me(AuthUser(u): AuthUser) -> Json<User> {
    Json(u)
}

#[derive(Deserialize)]
pub struct ChangePasswordReq {
    pub old_password: String,
    pub new_password: String,
}

pub async fn change_password(
    State(st): State<AppState>,
    AuthUser(u): AuthUser,
    Json(req): Json<ChangePasswordReq>,
) -> ApiResult<Json<serde_json::Value>> {
    let hash: String = sqlx::query_scalar("SELECT password_hash FROM users WHERE id = $1")
        .bind(u.id)
        .fetch_one(&st.db)
        .await?;
    if !verify_password_async(req.old_password, hash).await {
        return Err(bad("current password is wrong"));
    }
    check_password(&req.new_password)?;
    let h = hash_password(&req.new_password)?;
    sqlx::query("UPDATE users SET password_hash = $2 WHERE id = $1")
        .bind(u.id)
        .bind(h)
        .execute(&st.db)
        .await?;
    Ok(Json(json!({})))
}

fn check_password(pw: &str) -> ApiResult<()> {
    if pw.len() < 8 {
        return Err(bad("password must be at least 8 characters"));
    }
    Ok(())
}

fn check_username(name: &str) -> ApiResult<()> {
    if name.is_empty()
        || name.len() > 64
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
    {
        return Err(bad("username: 1-64 characters, letters, digits, . _ -"));
    }
    Ok(())
}

/// Create a user with a private hashed root folder on the server's default
/// storage.
pub async fn create_user(
    st: &AppState,
    username: &str,
    password: &str,
    is_admin: bool,
) -> ApiResult<User> {
    check_username(username)?;
    check_password(password)?;
    let hash = hash_password(password)?;
    // Under the layout lock from the start: the default connections (made
    // by the first sign-up) and each private root's volume.
    let (mut tx, _layout) = crate::mounts::lock_layout(st).await?;
    let user: User = sqlx::query_as(
        "INSERT INTO users (username, password_hash, is_admin) VALUES ($1, $2, $3) RETURNING id, username, is_admin",
    )
    .bind(username)
    .bind(hash)
    .bind(is_admin)
    .fetch_one(&mut *tx)
    .await?;
    let default: Option<i64> =
        sqlx::query_scalar("SELECT id FROM connections WHERE is_default LIMIT 1")
            .fetch_optional(&mut *tx)
            .await?;
    let existing_default = default.is_some();
    let conn_id: i64 = match default {
        Some(id) => id,
        None => {
            // First user: create the server's connections, owned by them:
            // a plain-files folder and the default store for private roots.
            insert_local_connection(
                st,
                &mut tx,
                user.id,
                "Server files",
                &st.cfg.files_dir,
                false,
            )
            .await?;
            insert_local_connection(
                st,
                &mut tx,
                user.id,
                "Server storage",
                &st.cfg.default_store,
                true,
            )
            .await?
        }
    };
    let root_base = format!("users/{}", user.id);
    if existing_default {
        // e.g. an admin raw-mounted `users/<id>` before this user existed
        crate::mounts::check_storage_overlap(st, conn_id, &root_base, "hashed").await?;
    }
    let volume_id: i64 = sqlx::query_scalar(
        "INSERT INTO volumes (connection_id, base_path, mode) VALUES ($1, $2, 'hashed')
         ON CONFLICT (connection_id, base_path, mode) DO UPDATE SET mode = EXCLUDED.mode RETURNING id",
    )
    .bind(conn_id)
    .bind(&root_base)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO mounts (owner_id, path, parent_path, name, volume_id, journal_from)
         VALUES ($1, '', NULL, '', $2, (SELECT journal_seq FROM volumes WHERE id = $2))",
    )
    .bind(user.id)
    .bind(volume_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(user)
}

async fn insert_local_connection(
    st: &AppState,
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    owner: i64,
    name: &str,
    path: &str,
    is_default: bool,
) -> ApiResult<i64> {
    let cfg = ConnectionConfig::Local {
        path: path.to_string(),
    };
    cfg.create_local_root(&st.cfg.local_base)?;
    let op = cfg.operator(&st.cfg.local_base)?;
    let caps = Capabilities::detect(&cfg, &op, &st.cfg.local_base).await;
    let enc = st.encrypt_config(&cfg)?;
    let (storage_id, storage_folder) = cfg.location(&st.cfg.local_base)?;
    Ok(sqlx::query_scalar(
        "INSERT INTO connections (owner_id, name, kind, config_enc, capabilities, is_default, storage_id, storage_folder)
         VALUES ($1, $2, 'local', $3, $4, $5, $6, $7) RETURNING id",
    )
    .bind(owner)
    .bind(name)
    .bind(enc)
    .bind(sqlx::types::Json(caps))
    .bind(is_default)
    .bind(storage_id)
    .bind(storage_folder)
    .fetch_one(&mut **tx)
    .await?)
}

/// Create the first admin from STRATA_ADMIN_USER / STRATA_ADMIN_PASSWORD when
/// the database has no users.
pub async fn bootstrap_admin(st: &AppState) -> anyhow::Result<()> {
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM users")
        .fetch_one(&st.db)
        .await?;
    if count > 0 {
        return Ok(());
    }
    match (&st.cfg.admin_user, &st.cfg.admin_password) {
        (Some(u), Some(p)) => {
            create_user(st, u, p, true)
                .await
                .map_err(|e| anyhow::anyhow!("bootstrap admin: {e}"))?;
            tracing::info!("created admin user {u}");
        }
        _ => tracing::warn!("no users yet: set STRATA_ADMIN_USER and STRATA_ADMIN_PASSWORD"),
    }
    Ok(())
}

// ---- admin: users ----

pub async fn list_users(State(st): State<AppState>, _a: AdminUser) -> ApiResult<Json<Vec<User>>> {
    Ok(Json(
        sqlx::query_as("SELECT id, username, is_admin FROM users ORDER BY username")
            .fetch_all(&st.db)
            .await?,
    ))
}

#[derive(Deserialize)]
pub struct CreateUserReq {
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub is_admin: bool,
}

pub async fn admin_create_user(
    State(st): State<AppState>,
    _a: AdminUser,
    Json(req): Json<CreateUserReq>,
) -> ApiResult<Json<User>> {
    Ok(Json(
        create_user(&st, req.username.trim(), &req.password, req.is_admin).await?,
    ))
}

pub async fn admin_delete_user(
    State(st): State<AppState>,
    AdminUser(a): AdminUser,
    Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    if id == a.id {
        return Err(bad("you cannot delete yourself"));
    }
    let owns_default: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM connections WHERE owner_id = $1 AND is_default)",
    )
    .bind(id)
    .fetch_one(&st.db)
    .await?;
    if owns_default {
        return Err(bad("this user owns the server storage connection"));
    }
    // Mounts go with the user; volumes (and their data) stay. One transaction,
    // so a refused delete leaves the account exactly as it was.
    let mut tx = st.db.begin().await?;
    sqlx::query("DELETE FROM mounts WHERE owner_id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    let r = sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await;
    if let Err(sqlx::Error::Database(d)) = &r
        && matches!(d.code().as_deref(), Some("23503") | Some("23001"))
    {
        return Err(bad(
            "this user owns connections that still hold volumes; delete them first",
        ));
    }
    if r?.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }
    tx.commit().await?;
    st.mounts.remove(&id);
    Ok(Json(json!({})))
}

#[derive(Deserialize)]
pub struct SetPasswordReq {
    pub password: String,
}

pub async fn admin_set_password(
    State(st): State<AppState>,
    _a: AdminUser,
    Path(id): Path<i64>,
    Json(req): Json<SetPasswordReq>,
) -> ApiResult<Json<serde_json::Value>> {
    check_password(&req.password)?;
    let h = hash_password(&req.password)?;
    let r = sqlx::query("UPDATE users SET password_hash = $2 WHERE id = $1")
        .bind(id)
        .bind(h)
        .execute(&st.db)
        .await?;
    if r.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }
    sqlx::query("DELETE FROM sessions WHERE user_id = $1")
        .bind(id)
        .execute(&st.db)
        .await?;
    Ok(Json(json!({})))
}
