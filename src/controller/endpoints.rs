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
                let api: Api<Ephemeron> = Api::all(client.clone());
                let patch = if available {
                    let domain: &str = ctx.get_ref().domain.as_ref();
                    serde_json::json!({
                        "metadata": {
                            "annotations": {
                                "host": &format!("{}.{}", &name, domain),
                            },
                        },
                    })
                } else {
                    serde_json::json!({
                        "metadata": {
                            "annotations": {
                                "host": null,
                            },
                        },
                    })
                };

                api.patch(&name, &PatchParams::default(), &Patch::Merge(patch))
                    .await
                    .map_err(Error::HostAnnotation)?;

                conditions::set_available(eph, client, Some(available))
                    .await
                    .map_err(Error::UpdateCondition)?;

                Ok(Some(Action::await_change()))
            }
        }
    } else {
        Ok(Some(Action::requeue(Duration::from_secs(2))))
    }
}
