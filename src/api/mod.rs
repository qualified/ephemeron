// Simple Web API for Ephemeron.
use std::{convert::Infallible, error::Error, sync::Arc};

use kube::Client;
use warp::{http::StatusCode, reply, Filter, Rejection, Reply};

mod auth;
mod handlers;

#[derive(Debug, serde::Deserialize, Clone)]
pub struct Config {
    /// Predefined services.
    pub presets: Presets,
    /// Map of known `app`s to its `key`s.
    pub apps: auth::Apps,
}

pub type Presets = std::collections::BTreeMap<String, crate::EphemeronService>;

/// Payload for creating service with a preset.
#[derive(serde::Deserialize, Debug, PartialEq, Clone)]
struct PresetPayload {
    /// The name of the preset to use.
    pub preset: String,
    /// The duration to expire the service after.
    pub duration: String,
}

/// Payload for patching expiry.
#[derive(serde::Deserialize, Debug, PartialEq, Clone)]
struct PatchPayload {
    /// The new duration to expire after.
    pub duration: String,
}

#[derive(serde::Serialize)]
struct ErrorMessage {
    message: String,
}

fn json_response<T: serde::Serialize>(res: &T, status: warp::http::StatusCode) -> reply::Response {
    reply::with_status(reply::json(res), status).into_response()
}

fn json_error_response<T: Into<String>>(
    message: T,
    status: warp::http::StatusCode,
) -> reply::Response {
    reply::with_status(
        reply::json(&ErrorMessage {
            message: message.into(),
        }),
        status,
    )
    .into_response()
}

#[must_use]
pub fn new(
    client: Client,
    config: Config,
) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
    let presets = Arc::new(config.presets);
    let apps = Arc::new(config.apps);
    healthz()
        .or(authenticate(apps))
        .or(create(client.clone(), presets))
        .or(get(client.clone()))
        .or(patch(client.clone()))
        .or(delete(client))
        .recover(handle_rejection)
}

// GET /
fn healthz() -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
    warp::get().and(warp::path::end().map(|| "OK"))
}

// POST /
fn create(
    client: Client,
    presets: Arc<Presets>,
) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
    warp::post()
        .and(warp::path::end())
        .and(auth::filter::with_authorization())
        .and(json_body::<PresetPayload>())
        .and(warp::any().map(move || presets.clone()))
        .and(with_client(client))
        .and_then(handlers::create)
}

// PATCH /:id
fn patch(client: Client) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
    warp::patch()
        .and(warp::path::param::<String>())
        .and(warp::path::end())
        .and(auth::filter::with_authorization())
        .and(json_body::<PatchPayload>())
        .and(with_client(client))
        .and_then(handlers::patch)
}

// GET /:id
fn get(client: Client) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
    warp::get()
        .and(warp::path::param::<String>())
        .and(warp::path::end())
        .and(auth::filter::with_authorization())
        .and(with_client(client))
        .and_then(handlers::get)
}

// DELETE /:id
fn delete(client: Client) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
    warp::delete()
        .and(warp::path::param::<String>())
        .and(warp::path::end())
        .and(auth::filter::with_authorization())
        .and(with_client(client))
        .and_then(handlers::delete)
}

// POST /auth
fn authenticate(
    apps: Arc<auth::Apps>,
) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
    warp::post()
        .and(warp::path("auth"))
        .and(warp::path::end())
        .and(warp::any().map(move || apps.clone()))
        .and(json_body::<auth::TokenRequest>())
        .and_then(auth::token)
}

fn with_client(client: Client) -> impl Filter<Extract = (Client,), Error = Infallible> + Clone {
    warp::any().map(move || client.clone())
}

fn json_body<T>() -> impl Filter<Extract = (T,), Error = Rejection> + Clone
where
    T: serde::de::DeserializeOwned + Send,
{
    warp::body::content_length_limit(1024 * 1024).and(warp::body::json())
}

#[allow(clippy::unused_async)]
async fn handle_rejection(err: Rejection) -> Result<impl Reply, Rejection> {
    let (message, status) = if err.is_not_found() {
        ("Not Found", StatusCode::NOT_FOUND)
    } else if err.find::<auth::filter::Error>().is_some() {
        ("Unauthorized", StatusCode::UNAUTHORIZED)
    } else if let Some(e) = err.find::<warp::filters::body::BodyDeserializeError>() {
        // TODO Improve error message. e.g., "missing field `duration`"
        if let Some(cause) = e.source() {
            tracing::debug!("deserialize error: {:?}", cause);
        }
        ("Bad Request", StatusCode::BAD_REQUEST)
    } else if err.find::<warp::reject::PayloadTooLarge>().is_some() {
        ("Payload Too Large", StatusCode::PAYLOAD_TOO_LARGE)
    } else if err.find::<warp::reject::MethodNotAllowed>().is_some() {
        ("Method Not Allowed", StatusCode::METHOD_NOT_ALLOWED)
    } else {
        tracing::warn!("unhandled rejection: {:?}", err);
        ("Internal Server Error", StatusCode::INTERNAL_SERVER_ERROR)
    };

    Ok(json_error_response(message, status))
}
