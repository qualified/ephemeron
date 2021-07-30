// authz/authn
use std::{collections::BTreeMap, convert::Infallible, sync::Arc};

use jsonwebtoken as jwt;
use once_cell::sync::Lazy;
use snafu::{OptionExt, ResultExt, Snafu};
use warp::{http::StatusCode, Reply};

use super::{json_error_response, json_response};

pub mod filter;

// Map of allowed apps and its api key (plain text).
// Loaded on startup from config file, and passed to token handler.
pub type Apps = BTreeMap<String, String>;

static JWT_SECRET: Lazy<String> =
    Lazy::new(|| std::env::var("JWT_SECRET").expect("JWT_SECRET is set"));

#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display("unknown app"))]
    AppLookup,

    #[snafu(display("invalid key"))]
    InvalidKey,

    #[snafu(display("failed to create token: {}", source))]
    CreateToken { source: jwt::errors::Error },
}

impl warp::Reply for Error {
    fn into_response(self) -> warp::reply::Response {
        match self {
            Error::InvalidKey | Error::AppLookup => {
                json_error_response("Unauthorized".to_owned(), StatusCode::UNAUTHORIZED)
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
    pub sub: String,
    pub exp: usize,
}

#[derive(Debug, serde::Deserialize)]
pub struct TokenRequest {
    app: String,
    key: String,
    uid: String,
}

#[derive(Debug, serde::Serialize)]
pub struct TokenResponse {
    token: String,
}

// `POST /auth` `{app: String, key: String, uid: String}` -> `{token: String}`
// Get short-lived token for frontend usage (backend app authenticates on behalf of its user).
// `uid` must be a string that's unique within `app`.
// The token's subject is `{uid}@{app}`, and it's valid for 5 minutes.
// The api key must be kept secret.
// Use this token to make requests to create and update resources.
#[allow(clippy::unused_async)]
pub async fn token(apps: Arc<Apps>, request: TokenRequest) -> Result<impl Reply, Infallible> {
    let key = match apps.get(&request.app).context(AppLookup) {
        Err(err) => return Ok(err.into_response()),
        Ok(key) => key,
    };
    if request.key != *key {
        return Ok(Error::InvalidKey.into_response());
    }

    let sub = format!("{}@{}", request.uid, request.app);
    let token = match create_jwt(&sub) {
        Err(err) => return Ok(err.into_response()),
        Ok(token) => token,
    };

    Ok(json_response(&TokenResponse { token }, StatusCode::OK))
}

fn create_jwt(sub: &str) -> Result<String, Error> {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let exp = chrono::Utc::now()
        .checked_add_signed(chrono::Duration::minutes(5))
        .expect("valid timestamp")
        .timestamp() as usize;

    jwt::encode(
        &jwt::Header::default(),
        &Claims {
            sub: sub.to_owned(),
            exp,
        },
        &jwt::EncodingKey::from_secret(JWT_SECRET.as_bytes()),
    )
    .context(CreateToken)
}
