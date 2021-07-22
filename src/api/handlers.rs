use std::convert::Infallible;

use kube::{
    api::{DeleteParams, PostParams, PropagationPolicy},
    Api, Client,
};
use serde::Serialize;
use warp::{http::StatusCode, Reply};

use crate::{Ephemeron, EphemeronSpec};

#[derive(Serialize)]
pub(crate) struct Created {
    id: String,
}

#[derive(Serialize)]
pub(crate) struct HostInfo {
    host: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct ErrorMessage {
    message: String,
}

#[tracing::instrument(skip(client), level = "debug")]
pub(crate) async fn create_with_preset(
    payload: super::PresetPayload,
    presets: super::Presets,
    client: Client,
) -> Result<impl Reply, Infallible> {
    if let Some(preset) = presets.get(&payload.preset) {
        if let Some(duration) = get_duration(&payload.duration) {
            let preset = preset.clone();
            let spec = EphemeronSpec {
                expires: chrono::Utc::now() + duration,
                service: preset,
            };
            let id = xid::new().to_string();
            let eph = Ephemeron::new(&id, spec);
            let api: Api<Ephemeron> = Api::all(client);
            tracing::trace!("creating");
            match api.create(&PostParams::default(), &eph).await {
                Ok(_) => {
                    tracing::trace!("created");
                    let json = warp::reply::json(&Created { id });
                    Ok(warp::reply::with_status(json, StatusCode::ACCEPTED))
                }
                Err(err) => Ok(error_json(err)),
            }
        } else {
            Ok(warp::reply::with_status(
                warp::reply::json(&ErrorMessage {
                    message: format!("invalid duration {}", payload.duration),
                }),
                StatusCode::BAD_REQUEST,
            ))
        }
    } else {
        Ok(warp::reply::with_status(
            warp::reply::json(&ErrorMessage {
                message: format!("preset {} not found", payload.preset),
            }),
            StatusCode::NOT_FOUND,
        ))
    }
}

#[tracing::instrument(skip(client), level = "debug")]
pub(crate) async fn get_host(id: String, client: Client) -> Result<impl Reply, Infallible> {
    let api: Api<Ephemeron> = Api::all(client);
    match api.get(&id).await {
        Ok(eph) => {
            let host = eph.metadata.annotations.get("host").cloned();
            let json = warp::reply::json(&HostInfo { host });
            Ok(warp::reply::with_status(json, StatusCode::OK))
        }
        Err(err) => Ok(error_json(err)),
    }
}

#[tracing::instrument(skip(client), level = "debug")]
pub(crate) async fn delete(id: String, client: Client) -> Result<impl Reply, Infallible> {
    let api: Api<Ephemeron> = Api::all(client);
    let dp = DeleteParams {
        propagation_policy: Some(PropagationPolicy::Background),
        ..DeleteParams::default()
    };
    match api.delete(&id, &dp).await {
        Ok(_) => Ok(StatusCode::NO_CONTENT),

        Err(kube::Error::Api(err)) => {
            Ok(StatusCode::from_u16(err.code).unwrap_or(StatusCode::BAD_REQUEST))
        }

        Err(err) => {
            tracing::warn!("Unexpected Error: {:?}", err);
            Ok(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

fn error_json(err: kube::Error) -> warp::reply::WithStatus<warp::reply::Json> {
    match err {
        kube::Error::Api(err) => {
            tracing::debug!("Kube Api error: {:?}", err);
            let status = StatusCode::from_u16(err.code).unwrap_or(StatusCode::BAD_REQUEST);
            let json = warp::reply::json(&ErrorMessage {
                message: err.message,
            });
            warp::reply::with_status(json, status)
        }

        err => {
            tracing::warn!("Unexpected Error: {:?}", err);
            let status = StatusCode::INTERNAL_SERVER_ERROR;
            let json = warp::reply::json(&ErrorMessage {
                message: status.canonical_reason().unwrap().to_owned(),
            });
            warp::reply::with_status(json, status)
        }
    }
}

fn get_duration(duration: &str) -> Option<chrono::Duration> {
    humantime::parse_duration(&duration)
        .ok()
        .and_then(|d| chrono::Duration::from_std(d).ok())
}
