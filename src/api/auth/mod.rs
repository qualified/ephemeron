// authz/authn
use std::{collections::BTreeMap, convert::Infallible, sync::Arc};

use jsonwebtoken as jwt;
use once_cell::sync::Lazy;
use thiserror::Error;
use warp::{http::StatusCode, Reply};

use super::{json_error_response, json_response};

pub mod filter;

// Map of allowed apps and its api key (plain text).
// Loaded on startup from config file, and passed to token handler.
pub type Apps = BTreeMap<String, String>;

static JWT_SECRET: Lazy<String> =
    Lazy::new(|| std::env::var("JWT_SECRET").expect("JWT_SECRET is set"));

#[derive(Debug, Error)]
pub enum Error {
    #[error("unknown app")]
    AppLookup,

    #[error("invalid key")]
    InvalidKey,

    #[error("invalid uid")]
    InvalidUserId,

    #[error("invalid gid")]
    InvalidGroupId,

    #[error("failed to create token: {0}")]
    CreateToken(#[source] jwt::errors::Error),
}

impl warp::Reply for Error {
    fn into_response(self) -> warp::reply::Response {
        match self {
            Error::InvalidKey | Error::AppLookup => {
                json_error_response("Unauthorized".to_owned(), StatusCode::UNAUTHORIZED)
            }

            Error::InvalidUserId => {
                json_error_response("Invalid uid".to_owned(), StatusCode::BAD_REQUEST)
            }

            Error::InvalidGroupId => {
                json_error_response("Invalid gid".to_owned(), StatusCode::BAD_REQUEST)
            }

            Error::CreateToken { .. } => json_error_response(
                "Internal Server Error".to_owned(),
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
        }
    }
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct Claims {
    /// Subject of the JWT. `uid.app`
    pub sub: String,
    /// Expiration time.
    pub exp: usize,
    /// Optional group id. `gid.app`
    pub gid: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
pub struct TokenRequest {
    /// The id of the app authenticating its user.
    app: String,
    /// The API key for the app.
    key: String,
    /// The id of the user of the app. Must be unique within the app.
    uid: String,
    /// Optional id of the group user belongs to. Must be unique within the app.
    gid: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct TokenResponse {
    token: String,
}

// `POST /auth` `{app: String, key: String, uid: String, gid?: String}` -> `{token: String}`
// Get short-lived token for frontend usage (backend app authenticates on behalf of its user).
// `uid` must be a string that's unique within `app`.
// The token's subject is `{uid}.{app}`, and it's valid for 5 minutes.
// The api key must be kept secret.
// Use this token to make requests to create and update resources.
#[allow(clippy::unused_async)]
pub async fn token(apps: Arc<Apps>, request: TokenRequest) -> Result<impl Reply, Infallible> {
    let key = match apps.get(&request.app).ok_or(Error::AppLookup) {
        Err(err) => return Ok(err.into_response()),
        Ok(key) => key,
    };
    if request.key != *key {
        return Ok(Error::InvalidKey.into_response());
    }
    // Label values must be 63 characters or less.
    let max_id_len = 63 - (request.app.len() + 1);
    if !is_valid_id(&request.uid, max_id_len) {
        return Ok(Error::InvalidUserId.into_response());
    }

    let sub = format!("{}.{}", request.uid, request.app);
    let gid = if let Some(gid) = request.gid {
        if !is_valid_id(&gid, max_id_len) {
            return Ok(Error::InvalidGroupId.into_response());
        }
        Some(format!("{}.{}", gid, request.app))
    } else {
        None
    };
    let token = match create_jwt(sub, gid) {
        Err(err) => return Ok(err.into_response()),
        Ok(token) => token,
    };

    Ok(json_response(&TokenResponse { token }, StatusCode::OK))
}

fn is_valid_id(s: &str, n: usize) -> bool {
    !s.is_empty() && s.len() <= n && s.chars().all(|c| c.is_ascii_alphanumeric())
}

fn create_jwt(sub: String, gid: Option<String>) -> Result<String, Error> {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let exp = chrono::Utc::now()
        .checked_add_signed(chrono::Duration::minutes(5))
        .expect("valid timestamp")
        .timestamp() as usize;

    jwt::encode(
        &jwt::Header::default(),
        &Claims { sub, exp, gid },
        &jwt::EncodingKey::from_secret(JWT_SECRET.as_bytes()),
    )
    .map_err(Error::CreateToken)
}
