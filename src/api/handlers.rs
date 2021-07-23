use std::convert::Infallible;

use chrono::{DateTime, Utc};
use kube::{
    api::{DeleteParams, Patch, PatchParams, PostParams, PropagationPolicy},
    Api, Client,
};
use snafu::{OptionExt, ResultExt, Snafu};
use warp::{http::StatusCode, reply, Reply};

use crate::{Ephemeron, EphemeronSpec};

#[derive(Debug, Snafu)]
pub(super) enum Error {
    #[snafu(display("preset {} not found", name))]
    PresetLookup { name: String },

    #[snafu(display("duration {} is invalid", duration))]
    InvalidDuration { duration: String },

    #[snafu(display("failed to parse duration {}", duration))]
    ParseDuration {
        duration: String,
        source: humantime::DurationError,
    },

    #[snafu(display("failed to create resource: {}", source))]
    CreateResource { source: kube::Error },

    #[snafu(display("failed to update resouce duration: {}", source))]
    PatchDuration { source: kube::Error },

    #[snafu(display("failed to get resource: {}", source))]
    GetResource { source: kube::Error },

    #[snafu(display("failed to delete: {}", source))]
    DeleteResource { source: kube::Error },
}

impl Reply for Error {
    fn into_response(self) -> reply::Response {
        #[allow(clippy::match_same_arms)]
        match self {
            err @ Error::PresetLookup { .. } => {
                json_error_response(err.to_string(), StatusCode::NOT_FOUND)
            }
            err @ Error::ParseDuration { .. } => {
                json_error_response(err.to_string(), StatusCode::BAD_REQUEST)
            }
            err @ Error::InvalidDuration { .. } => {
                json_error_response(err.to_string(), StatusCode::BAD_REQUEST)
            }

            Error::GetResource { source }
            | Error::CreateResource { source }
            | Error::PatchDuration { source } => match source {
                kube::Error::Api(err) => {
                    tracing::debug!("Kube Api error: {:?}", err);
                    json_error_response(
                        err.message,
                        StatusCode::from_u16(err.code).unwrap_or(StatusCode::BAD_REQUEST),
                    )
                }

                err => {
                    tracing::warn!("Unexpected Error: {:?}", err);
                    json_error_response(
                        "Internal Server Error".to_owned(),
                        StatusCode::INTERNAL_SERVER_ERROR,
                    )
                }
            },

            Error::DeleteResource { source } => match source {
                kube::Error::Api(err) => StatusCode::from_u16(err.code)
                    .unwrap_or(StatusCode::BAD_REQUEST)
                    .into_response(),

                err => {
                    tracing::warn!("Unexpected Error: {:?}", err);
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
            },
        }
    }
}

#[derive(serde::Serialize)]
struct Created {
    id: String,
}

#[derive(serde::Serialize)]
struct HostInfo {
    host: Option<String>,
    expires: DateTime<Utc>,
}

#[derive(serde::Serialize)]
struct Expiration {
    expires: DateTime<Utc>,
}

#[derive(serde::Serialize)]
struct ErrorMessage {
    message: String,
}

// Use this instead of `?` to avoid rejecting.
macro_rules! warp_try {
    ($expr:expr) => {
        match $expr {
            Ok(val) => val,
            Err(err) => {
                return Ok(err.into_response());
            }
        }
    };
}

#[tracing::instrument(skip(client), level = "debug")]
pub(super) async fn create(
    payload: super::PresetPayload,
    presets: super::Presets,
    client: Client,
) -> Result<impl Reply, Infallible> {
    let preset = warp_try!(presets.get(&payload.preset).with_context(|| PresetLookup {
        name: payload.preset.clone(),
    }));
    let duration = warp_try!(get_duration(&payload.duration));

    let id = xid::new().to_string();
    let eph = Ephemeron::new(
        &id,
        EphemeronSpec {
            expires: chrono::Utc::now() + duration,
            service: preset.clone(),
        },
    );
    let api: Api<Ephemeron> = Api::all(client);
    tracing::trace!("creating");
    let _res = warp_try!(api
        .create(&PostParams::default(), &eph)
        .await
        .context(CreateResource));
    tracing::trace!("created");
    Ok(json_response(&Created { id }, StatusCode::ACCEPTED))
}

#[tracing::instrument(skip(client), level = "debug")]
pub(super) async fn patch(
    id: String,
    payload: super::PatchPayload,
    client: Client,
) -> Result<impl Reply, Infallible> {
    let duration = warp_try!(get_duration(&payload.duration));
    let patch = Patch::Merge(serde_json::json!({
        "spec": {
            "expires": chrono::Utc::now() + duration,
        },
    }));
    let api: Api<Ephemeron> = Api::all(client);
    let eph = warp_try!(api
        .patch(&id, &PatchParams::default(), &patch)
        .await
        .context(PatchDuration));
    Ok(json_response(
        &Expiration {
            expires: eph.spec.expires,
        },
        StatusCode::OK,
    ))
}

#[tracing::instrument(skip(client), level = "debug")]
pub(super) async fn get(id: String, client: Client) -> Result<impl Reply, Infallible> {
    let api: Api<Ephemeron> = Api::all(client);
    let eph = warp_try!(api.get(&id).await.context(GetResource));
    Ok(json_response(
        &HostInfo {
            host: eph.metadata.annotations.get("host").cloned(),
            expires: eph.spec.expires,
        },
        StatusCode::OK,
    ))
}

#[tracing::instrument(skip(client), level = "debug")]
pub(super) async fn delete(id: String, client: Client) -> Result<impl Reply, Infallible> {
    let api: Api<Ephemeron> = Api::all(client);
    let dp = DeleteParams {
        propagation_policy: Some(PropagationPolicy::Background),
        ..DeleteParams::default()
    };
    let _res = warp_try!(api.delete(&id, &dp).await.context(DeleteResource));
    Ok(StatusCode::NO_CONTENT.into_response())
}

fn get_duration(duration: &str) -> Result<chrono::Duration, Error> {
    humantime::parse_duration(duration)
        .with_context(|| ParseDuration {
            duration: duration.to_owned(),
        })
        .and_then(|d| {
            chrono::Duration::from_std(d).map_err(|_| Error::InvalidDuration {
                duration: duration.to_owned(),
            })
        })
}

fn json_response<T: serde::Serialize>(res: &T, status: StatusCode) -> reply::Response {
    reply::with_status(reply::json(res), status).into_response()
}

fn json_error_response(message: String, status: StatusCode) -> reply::Response {
    reply::with_status(reply::json(&ErrorMessage { message }), status).into_response()
}
