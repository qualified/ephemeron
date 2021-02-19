use std::collections::BTreeMap;

use chrono::Utc;
use futures::StreamExt;
use k8s_openapi::{
    api::{
        core::v1::{Pod, Service},
        networking::v1::Ingress,
    },
    apimachinery::pkg::apis::meta::v1::OwnerReference,
    Resource,
};
use kube::{
    api::{ListParams, Meta},
    Api, Client,
};
use kube_runtime::controller::{Context, Controller, ReconcilerAction};
use snafu::{ResultExt, Snafu};
use tracing::{trace, warn};

use super::Ephemeron;
mod conditions;
mod endpoints;
mod expiry;
mod ingress;
mod pod;
mod service;

const PROJECT_NAME: &str = "ephemeron";
#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display("Failed to delete expired resource: {}", source))]
    DeleteExpired { source: expiry::Error },

    #[snafu(display("Failed to reconcile pod: {}", source))]
    ReconcilePod { source: pod::Error },

    #[snafu(display("Failed to reconcile service: {}", source))]
    ReconcileService { source: service::Error },

    #[snafu(display("Failed to reconcile ingress: {}", source))]
    ReconcileIngress { source: ingress::Error },

    #[snafu(display("Failed to reconcile endpoints: {}", source))]
    ReconcileEndpoints { source: endpoints::Error },
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
    if let Some(conditions) = eph.status.as_ref().map(|s| &s.conditions) {
        trace!("conditions: {:?}", conditions);
    }

    if let Some(action) = expiry::delete_expired(&eph, ctx.clone())
        .await
        .context(DeleteExpired)?
    {
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
    if let Some(action) = endpoints::reconcile(&eph, ctx.clone())
        .await
        .context(ReconcileEndpoints)?
    {
        return Ok(action);
    }

    // Nothing happened in this loop, so the resource is in the desired state.
    // Requeue around when this expires unless something else triggers reconciliation.
    Ok(ReconcilerAction {
        requeue_after: Some((eph.spec.expires - Utc::now()).to_std().unwrap_or_default()),
    })
}

#[allow(clippy::needless_pass_by_value)]
/// An error handler called when the reconciler fails.
fn error_policy(error: &Error, _ctx: Context<ContextData>) -> ReconcilerAction {
    warn!("reconciler failed: {}", error);
    ReconcilerAction {
        requeue_after: None,
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
