// Provides simple Web API for Ephemeron.
//
// Routes:
//
// - `POST /`: Create new service based on a preset and duration string. Responds with JSON `{"id": ""}`.
// - `GET /{id}`: Get the host. Responds with JSON `{"host": "", "expires": timestamp}`.
//   `host` is `null` unless `Available`.
// - `PATCH /{id}`: Change when the resource expires with a duration string.
// - `DELETE /{id}`: Delete the resource and all of its children
//
// TODO Authentication
// TODO Authorization: Only the user who created the resource can change them
// TODO Allow extending lifetime
use std::convert::Infallible;

use kube::Client;
use warp::{Filter, Rejection, Reply};

mod config;
mod handlers;

pub use config::{Config, PresetPayload, Presets};

/// Payload for patching expiry.
#[derive(serde::Deserialize, Debug, PartialEq, Clone)]
pub struct PatchPayload {
    /// The new duration to expire after.
    pub duration: String,
}

#[must_use]
pub fn new(
    client: Client,
    config: Option<Config>,
) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
    let presets = config.map(|c| c.presets).unwrap_or_default();
    healthz()
        .or(create_with_preset(client.clone(), presets))
        .or(get_host(client.clone()))
        .or(patch(client.clone()))
        .or(delete(client))
}

// GET /
fn healthz() -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
    warp::path::end().and(warp::get()).map(|| "OK")
}

// POST /
fn create_with_preset(
    client: Client,
    presets: Presets,
) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
    warp::path::end()
        .and(warp::post())
        .and(json_body::<PresetPayload>())
        .and(warp::any().map(move || presets.clone()))
        .and(with_client(client))
        .and_then(handlers::create_with_preset)
}

// PATCH /:id
fn patch(client: Client) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
    warp::path::param::<String>()
        .and(warp::path::end())
        .and(warp::patch())
        .and(json_body::<PatchPayload>())
        .and(with_client(client))
        .and_then(handlers::patch)
}

// GET /:id
fn get_host(client: Client) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
    warp::path::param::<String>()
        .and(warp::path::end())
        .and(warp::get())
        .and(with_client(client))
        .and_then(handlers::get_host)
}

// DELETE /:id
fn delete(client: Client) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
    warp::path::param::<String>()
        .and(warp::path::end())
        .and(warp::delete())
        .and(with_client(client))
        .and_then(handlers::delete)
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
