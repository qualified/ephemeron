// Provides simple Web API for Ephemeron.
//
// Routes:
//
// - `POST /`: Create new resource. Responds with JSON `{"id": ""}`.
// - `GET /{id}` Get the host. Responds with JSON `{"host": ""}`. `host` is `null` unless `Available`.
// - `DELETE /{id}`: Delete the resource and all of its children
//
// Admin Routes:
//
// - `GET /`: List of resources
//
// TODO Authentication
// TODO Authorization: Only the user who created the resource can change them
// TODO Allow extending
use std::convert::Infallible;

use kube::Client;
use warp::{Filter, Rejection, Reply};

use crate::EphemeronSpec;
mod handlers;

#[must_use]
pub fn new(client: Client) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
    healthz()
        .or(create(client.clone()))
        .or(get_host(client.clone()))
        .or(delete(client))
}

// GET /
fn healthz() -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
    warp::get().and(warp::path::end()).map(|| "OK")
}

// POST /
fn create(client: Client) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
    warp::post()
        .and(warp::path::end())
        .and(json_body())
        .and(with_client(client))
        .and_then(handlers::create)
}

// GET /:id
fn get_host(client: Client) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
    warp::get()
        .and(warp::path::param::<String>())
        .and(warp::path::end())
        .and(with_client(client))
        .and_then(handlers::get_host)
}

// DELETE /:id
fn delete(client: Client) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
    warp::delete()
        .and(warp::path::param::<String>())
        .and(warp::path::end())
        .and(with_client(client))
        .and_then(handlers::delete)
}

fn with_client(client: Client) -> impl Filter<Extract = (Client,), Error = Infallible> + Clone {
    warp::any().map(move || client.clone())
}

fn json_body() -> impl Filter<Extract = (EphemeronSpec,), Error = Rejection> + Clone {
    warp::body::content_length_limit(1024 * 1024).and(warp::body::json())
}
