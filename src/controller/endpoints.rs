use std::time::Duration;

use k8s_openapi::api::core::v1::Endpoints;
use kube::{
    api::{Patch, PatchParams},
    runtime::controller::{Action, Context},
    Api, ResourceExt,
};
use thiserror::Error;

use super::{conditions, ContextData};
use crate::Ephemeron;

#[derive(Debug, Error)]
pub enum Error {
    #[error("failed to get endpoints: {0}")]
    GetEndpoints(#[source] kube::Error),

    #[error("failed to annotate host information: {0}")]
    HostAnnotation(#[source] kube::Error),

    #[error("failed to update condition: {0}")]
    UpdateCondition(#[source] conditions::Error),
}
pub type Result<T, E = Error> = std::result::Result<T, E>;

#[tracing::instrument(skip(eph, ctx), level = "trace")]
pub(super) async fn reconcile(
    eph: &Ephemeron,
    ctx: Context<ContextData>,
) -> Result<Option<Action>> {
    let name = eph.name();
    let client = ctx.get_ref().client.clone();
    // Check if service has endpoints
    let endpoints: Api<Endpoints> = Api::namespaced(client.clone(), super::NS);
    if let Some(Endpoints { subsets, .. }) = endpoints
        .get_opt(&name)
        .await
        .map_err(Error::GetEndpoints)?
    {
        let has_ready = subsets.map_or(false, |ess| {
            ess.iter()
                .any(|es| es.addresses.as_ref().map_or(false, |a| !a.is_empty()))
        });
        match (eph.is_available(), has_ready) {
            // Nothing to do if it's ready and the condition agrees.
            (true, true) => Ok(None),
            // Requeue soon if `Endpoints` exists, but not ready yet.
            (false, false) => Ok(Some(Action::requeue(Duration::from_secs(1)))),
            // Fix outdated condition
            (_, available) => {
                let host = if available {
                    // HACK Make sure the service is available from outside.
                    // The address is marked as ready to be utilized, but that doesn't mean it's usable from outside.
                    let domain: &str = ctx.get_ref().domain.as_ref();
                    let host = format!("{}.{}", &name, domain);
                    if let Some(probe) = eph.spec.service.readiness_probe.as_ref() {
                        let uri = hyper::Uri::builder()
                            .scheme("http")
                            .authority(host.clone())
                            .path_and_query(probe.path.clone())
                            .build()
                            .expect("valid uri from host");
                        tracing::debug!("testing if {} is available", uri);
                        let http_client = ctx.get_ref().http_client.clone();
                        match http_client.get(uri).await {
                            Ok(res) if res.status() == hyper::StatusCode::OK => {
                                tracing::debug!("the service is available");
                                Some(host)
                            }
                            Ok(res) => {
                                tracing::debug!(
                                    "the service is not available yet {}",
                                    res.status()
                                );
                                // Try again after 1s, or the next cycle.
                                return Ok(Some(Action::requeue(Duration::from_secs(1))));
                            }
                            Err(err) => {
                                tracing::warn!("failed to check availability {}", err);
                                None
                            }
                        }
                    } else {
                        Some(host)
                    }
                } else {
                    None
                };

                let api: Api<Ephemeron> = Api::all(client.clone());
                api.patch(
                    &name,
                    &PatchParams::default(),
                    &Patch::Merge(serde_json::json!({
                        "metadata": { "annotations": { "host": host } },
                    })),
                )
                .await
                .map_err(Error::HostAnnotation)?;

                conditions::set_available(eph, client, Some(host.is_some()))
                    .await
                    .map_err(Error::UpdateCondition)?;

                Ok(Some(Action::await_change()))
            }
        }
    } else {
        Ok(Some(Action::requeue(Duration::from_secs(2))))
    }
}
