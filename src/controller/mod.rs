use std::{collections::BTreeMap, time::Duration};

use chrono::Utc;
use futures::StreamExt;
use k8s_openapi::{
    api::{
        core::v1::{Endpoints, Pod, Service},
        networking::v1::Ingress,
    },
    apimachinery::pkg::apis::meta::v1::OwnerReference,
    Resource,
};
use kube::{
    api::{DeleteParams, ListParams, Meta, Patch, PatchParams, PropagationPolicy},
    error::ErrorResponse,
    Api, Client,
};
use kube_runtime::controller::{Context, Controller, ReconcilerAction};
use snafu::{ResultExt, Snafu};
use tracing::{debug, trace, warn};

use super::{Ephemeron, EphemeronStatus};
mod conditions;
mod ingress;
mod pod;
mod service;

const PROJECT_NAME: &str = "ephemeron";
#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display("Failed to get endpoints: {}", source))]
    GetEndpoints { source: kube::Error },

    #[snafu(display("Failed to delete ephemeron: {}", source))]
    Delete { source: kube::Error },

    #[snafu(display("Failed to annotate host information: {}", source))]
    HostAnnotation { source: kube::Error },

    #[snafu(display("Failed reconcile pod: {}", source))]
    ReconcilePod { source: pod::Error },

    #[snafu(display("Failed reconcile service: {}", source))]
    ReconcileService { source: service::Error },

    #[snafu(display("Failed reconcile ingress: {}", source))]
    ReconcileIngress { source: ingress::Error },

    #[snafu(display("Failed update condition: {}", source))]
    UpdateCondition { source: conditions::Error },
}
pub type Result<T, E = Error> = std::result::Result<T, E>;

// TODO Configurable
const NS: &str = "default";

pub async fn run(client: Client, domain: String) {
    let context = Context::new(ContextData {
        client: client.clone(),
        domain,
    });

    let lp = ListParams::default();
    Controller::<Ephemeron>::new(Api::all(client.clone()), lp.clone())
        .owns::<Pod>(Api::namespaced(client.clone(), NS), lp.clone())
        .owns::<Service>(Api::namespaced(client.clone(), NS), lp.clone())
        .owns::<Ingress>(Api::namespaced(client.clone(), NS), lp)
        .run(reconciler, error_policy, context)
        .filter_map(|x| async move { x.ok() })
        .for_each(|(_, action)| async move {
            trace!("Reconciled: requeue after {:?}", action.requeue_after);
        })
        .await;
}

// Data to store in context
struct ContextData {
    client: Client,
    domain: String,
}

#[tracing::instrument(skip(eph, ctx), level = "debug")]
async fn reconciler(eph: Ephemeron, ctx: Context<ContextData>) -> Result<ReconcilerAction> {
    if let Some(status) = eph.status.as_ref() {
        update_status(&eph, ctx, status).await
    } else {
        initialize_status(&eph, ctx).await
    }
}

#[allow(clippy::needless_pass_by_value)]
/// An error handler called when the reconciler fails.
fn error_policy(error: &Error, _ctx: Context<ContextData>) -> ReconcilerAction {
    warn!("reconciler failed: {}", error);
    ReconcilerAction {
        requeue_after: None,
    }
}

async fn initialize_status(eph: &Ephemeron, ctx: Context<ContextData>) -> Result<ReconcilerAction> {
    debug!("First reconciliation");
    debug!("Patching status");
    let client = ctx.get_ref().client.clone();
    conditions::set_pod_ready(&eph, client.clone(), None)
        .await
        .context(UpdateCondition)?;
    conditions::set_available(&eph, client, None)
        .await
        .context(UpdateCondition)?;

    Ok(ReconcilerAction {
        requeue_after: None,
    })
}

async fn update_status(
    eph: &Ephemeron,
    ctx: Context<ContextData>,
    status: &EphemeronStatus,
) -> Result<ReconcilerAction> {
    trace!("conditions: {:?}", status.conditions);

    if let Some(action) = delete_expired(&eph, ctx.clone()).await? {
        return Ok(action);
    }
    if let Some(action) = pod::reconcile(&eph, ctx.clone())
        .await
        .context(ReconcilePod)?
    {
        return Ok(action);
    }
    if let Some(action) = service::reconcile(&eph, ctx.clone())
        .await
        .context(ReconcileService)?
    {
        return Ok(action);
    }
    if let Some(action) = ingress::reconcile(&eph, ctx.clone())
        .await
        .context(ReconcileIngress)?
    {
        return Ok(action);
    }
    if let Some(action) = reconcile_endpoints(&eph, ctx.clone()).await? {
        return Ok(action);
    }

    // Nothing happened in this loop, so the resource is in the desired state.
    // Requeue around when this expires unless something else triggers reconciliation.
    debug!("Requeue later");
    Ok(ReconcilerAction {
        requeue_after: Some((eph.spec.expires - Utc::now()).to_std().unwrap_or_default()),
    })
}

/// Delete the resource if it's expired.
async fn delete_expired(
    eph: &Ephemeron,
    ctx: Context<ContextData>,
) -> Result<Option<ReconcilerAction>> {
    if eph.spec.expires > Utc::now() {
        return Ok(None);
    }

    debug!("Resource expired, deleting");
    let name = Meta::name(eph);
    // Delete the owner with `propagationPolicy=Background`.
    // This will delete the owner immediately, then children are deleted by garbage collector.
    let api: Api<Ephemeron> = Api::all(ctx.get_ref().client.clone());
    api.delete(
        &name,
        &DeleteParams {
            propagation_policy: Some(PropagationPolicy::Background),
            ..DeleteParams::default()
        },
    )
    .await
    .context(Delete)?;

    return Ok(Some(ReconcilerAction {
        requeue_after: None,
    }));
}

async fn reconcile_endpoints(
    eph: &Ephemeron,
    ctx: Context<ContextData>,
) -> Result<Option<ReconcilerAction>> {
    if eph.is_available() {
        return Ok(None);
    }

    let name = Meta::name(eph);
    let client = ctx.get_ref().client.clone();
    // Check if service has endpoints
    let endpoints: Api<Endpoints> = Api::namespaced(client.clone(), NS);
    match endpoints.get(&name).await {
        Ok(Endpoints {
            subsets: Some(ss), ..
        }) if ss.iter().any(|s| s.addresses.is_some()) => {
            let domain: &str = ctx.get_ref().domain.as_ref();
            let api: Api<Ephemeron> = Api::all(client.clone());
            api.patch(
                &name,
                &PatchParams::default(),
                &Patch::Merge(serde_json::json!({
                    "metadata": {
                        "annotations": {
                            "host": &format!("{}.{}", &name, domain),
                        },
                    },
                })),
            )
            .await
            .context(HostAnnotation)?;

            conditions::set_available(&eph, client, Some(true))
                .await
                .context(UpdateCondition)?;
            Ok(Some(ReconcilerAction {
                requeue_after: None,
            }))
        }

        Ok(_) | Err(kube::Error::Api(ErrorResponse { code: 404, .. })) => {
            Ok(Some(ReconcilerAction {
                requeue_after: Some(Duration::from_secs(1)),
            }))
        }

        Err(err) => Err(Error::GetEndpoints { source: err }),
    }
}

fn make_common_labels(name: &str) -> BTreeMap<String, String> {
    vec![
        ("app.kubernetes.io/name", name),
        ("app.kubernetes.io/managed-by", PROJECT_NAME),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect::<BTreeMap<_, _>>()
}

fn to_owner_reference(eph: &Ephemeron) -> OwnerReference {
    OwnerReference {
        api_version: <Ephemeron as Resource>::API_VERSION.to_string(),
        kind: <Ephemeron as Resource>::KIND.to_string(),
        name: Meta::name(eph),
        uid: eph.metadata.uid.clone().expect(".metadata.uid"),
        controller: Some(true),
        block_owner_deletion: Some(true),
    }
}
