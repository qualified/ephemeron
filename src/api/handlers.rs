use std::{convert::Infallible, sync::Arc};

use chrono::{DateTime, Utc};
use kube::{
    api::{DeleteParams, Patch, PatchParams, PostParams, PropagationPolicy},
    Api, Client, ResourceExt,
};
use snafu::{OptionExt, ResultExt, Snafu};
use warp::{http::StatusCode, reply, Reply};

use super::{json_error_response, json_response};
use crate::{Ephemeron, EphemeronSpec};

#[derive(Debug, Snafu)]
pub(super) enum Error {
    #[snafu(display("preset {} not found", name))]
    PresetLookup { name: String },

    #[snafu(display("lifetime {} is invalid", lifetime))]
    InvalidLifetime { lifetime: u32 },

    #[snafu(display("failed to create resource: {}", source))]
    CreateResource { source: kube::Error },

    #[snafu(display("failed to update resouce lifetime: {}", source))]
    PatchLifetime { source: kube::Error },

    #[snafu(display("failed to get resource: {}", source))]
    GetResource { source: kube::Error },

    #[snafu(display("failed to delete: {}", source))]
    DeleteResource { source: kube::Error },

    #[snafu(display("forbidden"))]
    Forbidden,
}

impl Reply for Error {
    fn into_response(self) -> reply::Response {
        #[allow(clippy::match_same_arms)]
        match self {
            err @ Error::PresetLookup { .. } => {
                json_error_response(err.to_string(), StatusCode::NOT_FOUND)
            }
            err @ Error::InvalidLifetime { .. } => {
                json_error_response(err.to_string(), StatusCode::BAD_REQUEST)
            }

            Error::Forbidden => json_error_response("Forbidden", StatusCode::FORBIDDEN),

            Error::GetResource { source }
            | Error::CreateResource { source }
            | Error::PatchLifetime { source } => match source {
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
#[serde(rename_all = "camelCase")]
struct Created {
    id: String,
    expiration_time: DateTime<Utc>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct HostInfo {
    host: Option<String>,
    expiration_time: DateTime<Utc>,
    tls: bool,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct Expiration {
    expiration_time: DateTime<Utc>,
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

const CREATED_BY: &str = "ephemerons.qualified.io/created-by";

#[tracing::instrument(skip(client, presets), level = "debug")]
pub(super) async fn create(
    claims: super::auth::Claims,
    payload: super::PresetPayload,
    presets: Arc<super::Presets>,
    client: Client,
) -> Result<impl Reply, Infallible> {
    let preset = warp_try!(presets.get(&payload.preset).with_context(|| PresetLookup {
        name: payload.preset.clone(),
    }));

    let duration = warp_try!(get_duration(payload.lifetime_minutes));
    let id = xid::new().to_string();
    let mut eph = Ephemeron::new(
        &id,
        EphemeronSpec {
            expiration_time: chrono::Utc::now() + duration,
            service: preset.clone(),
        },
    );
    eph.annotations_mut()
        .insert(CREATED_BY.to_owned(), claims.sub);

    let api: Api<Ephemeron> = Api::all(client);
    let eph = warp_try!(api
        .create(&PostParams::default(), &eph)
        .await
        .context(CreateResource));
    Ok(json_response(
        &Created {
            id,
            expiration_time: eph.spec.expiration_time,
        },
        StatusCode::ACCEPTED,
    ))
}

#[tracing::instrument(skip(client), level = "debug")]
pub(super) async fn patch(
    id: String,
    claims: super::auth::Claims,
    payload: super::PatchPayload,
    client: Client,
) -> Result<impl Reply, Infallible> {
    let api: Api<Ephemeron> = Api::all(client);
    let eph = warp_try!(api.get(&id).await.context(GetResource));
    if !has_access(&eph, &claims.sub) {
        return Ok(Error::Forbidden.into_response());
    }

    let duration = warp_try!(get_duration(payload.lifetime_minutes));
    let patch = Patch::Merge(serde_json::json!({
        "spec": {
            "expirationTime": chrono::Utc::now() + duration,
        },
    }));
    let eph = warp_try!(api
        .patch(&id, &PatchParams::default(), &patch)
        .await
        .context(PatchLifetime));
    Ok(json_response(
        &Expiration {
            expiration_time: eph.spec.expiration_time,
        },
        StatusCode::OK,
    ))
}

#[tracing::instrument(skip(client), level = "debug")]
pub(super) async fn get(
    id: String,
    claims: super::auth::Claims,
    client: Client,
) -> Result<impl Reply, Infallible> {
    let api: Api<Ephemeron> = Api::all(client);
    let eph = warp_try!(api.get(&id).await.context(GetResource));
    if !has_access(&eph, &claims.sub) {
        return Ok(Error::Forbidden.into_response());
    }

    Ok(json_response(
        &HostInfo {
            host: eph.annotations().get("host").cloned(),
            expiration_time: eph.spec.expiration_time,
            tls: eph.spec.service.tls_secret_name.is_some(),
        },
        StatusCode::OK,
    ))
}

#[tracing::instrument(skip(client), level = "debug")]
pub(super) async fn delete(
    id: String,
    claims: super::auth::Claims,
    client: Client,
) -> Result<impl Reply, Infallible> {
    let api: Api<Ephemeron> = Api::all(client);
    let eph = warp_try!(api.get(&id).await.context(GetResource));
    if !has_access(&eph, &claims.sub) {
        return Ok(Error::Forbidden.into_response());
    }

    let dp = DeleteParams {
        propagation_policy: Some(PropagationPolicy::Background),
        ..DeleteParams::default()
    };
    let _res = warp_try!(api.delete(&id, &dp).await.context(DeleteResource));
    Ok(StatusCode::NO_CONTENT.into_response())
}

fn get_duration(minutes: u32) -> Result<chrono::Duration, Error> {
    let duration = std::time::Duration::from_secs((minutes * 60).into());
    chrono::Duration::from_std(duration).map_err(|_| Error::InvalidLifetime { lifetime: minutes })
}

fn has_access(eph: &Ephemeron, sub: &str) -> bool {
    eph.annotations()
        .get(CREATED_BY)
        .map_or(false, |by| by == sub)
}
