use std::time::Duration;

use k8s_openapi::api::core::v1::Endpoints;
use kube::{
    api::{Patch, PatchParams},
    error::ErrorResponse,
    Api, ResourceExt,
};
use kube_runtime::controller::{Context, ReconcilerAction};
use snafu::{ResultExt, Snafu};

use super::{conditions, ContextData};
use crate::Ephemeron;

#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display("Failed to get endpoints: {}", source))]
    GetEndpoints { source: kube::Error },

    #[snafu(display("Failed to annotate host information: {}", source))]
    HostAnnotation { source: kube::Error },

    #[snafu(display("Failed to update condition: {}", source))]
    UpdateCondition { source: conditions::Error },
}
pub type Result<T, E = Error> = std::result::Result<T, E>;

#[tracing::instrument(skip(eph, ctx), level = "trace")]
pub(super) async fn reconcile(
    eph: &Ephemeron,
    ctx: Context<ContextData>,
) -> Result<Option<ReconcilerAction>> {
    let name = eph.name();
    let client = ctx.get_ref().client.clone();
    // Check if service has endpoints
    let endpoints: Api<Endpoints> = Api::namespaced(client.clone(), super::NS);
    match endpoints.get(&name).await {
        Ok(Endpoints { subsets, .. }) => {
            let has_ready = subsets.map_or(false, |ess| {
                ess.iter()
                    .any(|es| es.addresses.as_ref().map_or(false, |a| !a.is_empty()))
            });
            match (eph.is_available(), has_ready) {
                // Nothing to do if it's ready and the condition agrees.
                (true, true) => Ok(None),
                // Requeue soon if `Endpoints` exists, but not ready yet.
                (false, false) => Ok(Some(ReconcilerAction {
                    requeue_after: Some(Duration::from_secs(1)),
                })),
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
                        .context(HostAnnotation)?;

                    conditions::set_available(eph, client, Some(available))
                        .await
                        .context(UpdateCondition)?;

                    Ok(Some(ReconcilerAction {
                        requeue_after: None,
                    }))
                }
            }
        }

        Err(kube::Error::Api(ErrorResponse { code: 404, .. })) => Ok(Some(ReconcilerAction {
            requeue_after: Some(Duration::from_secs(2)),
        })),

        Err(err) => Err(Error::GetEndpoints { source: err }),
    }
}
