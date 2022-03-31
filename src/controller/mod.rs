use std::{collections::BTreeMap, sync::Arc};

use chrono::Utc;
use futures::StreamExt;
use k8s_openapi::{
    api::{
        core::v1::{Pod, Service},
        networking::v1::Ingress,
    },
    apimachinery::pkg::apis::meta::v1::OwnerReference,
};
use kube::{
    api::ListParams,
    runtime::controller::{Action, Context, Controller},
    Api, Client, Resource, ResourceExt,
};
use thiserror::Error;

use super::Ephemeron;
mod conditions;
mod endpoints;
mod expiry;
mod ingress;
mod pod;
mod service;

const PROJECT_NAME: &str = "ephemeron";
#[derive(Debug, Error)]
pub enum Error {
    #[error("failed to delete expired resource: {0}")]
    DeleteExpired(#[source] expiry::Error),

    #[error("failed to reconcile pod: {0}")]
    ReconcilePod(#[source] pod::Error),

    #[error("failed to reconcile service: {0}")]
    ReconcileService(#[source] service::Error),

    #[error("failed to reconcile ingress: {0}")]
    ReconcileIngress(#[source] ingress::Error),

    #[error("failed to reconcile endpoints: {0}")]
    ReconcileEndpoints(#[source] endpoints::Error),
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
            tracing::trace!("Reconciled: {:?}", action);
        })
        .await;
}

// Data to store in context
struct ContextData {
    client: Client,
    domain: String,
}

#[tracing::instrument(skip(eph, ctx), level = "trace")]
async fn reconciler(eph: Arc<Ephemeron>, ctx: Context<ContextData>) -> Result<Action> {
    if let Some(conditions) = eph.status.as_ref().map(|s| &s.conditions) {
        tracing::trace!("conditions: {:?}", conditions);
    }

    if let Some(action) = expiry::reconcile(&eph, ctx.clone())
        .await
        .map_err(Error::DeleteExpired)?
    {
        return Ok(action);
    }
    if let Some(action) = pod::reconcile(&eph, ctx.clone())
        .await
        .map_err(Error::ReconcilePod)?
    {
        return Ok(action);
    }
    if let Some(action) = service::reconcile(&eph, ctx.clone())
        .await
        .map_err(Error::ReconcileService)?
    {
        return Ok(action);
    }
    if let Some(action) = ingress::reconcile(&eph, ctx.clone())
        .await
        .map_err(Error::ReconcileIngress)?
    {
        return Ok(action);
    }
    if let Some(action) = endpoints::reconcile(&eph, ctx.clone())
        .await
        .map_err(Error::ReconcileEndpoints)?
    {
        return Ok(action);
    }

    // Nothing happened in this loop, so the resource is in the desired state.
    // Requeue around when this expires unless something else triggers reconciliation.
    Ok(Action::requeue(
        (eph.spec.expiration_time - Utc::now())
            .to_std()
            .unwrap_or_default(),
    ))
}

#[allow(clippy::needless_pass_by_value)]
/// An error handler called when the reconciler fails.
fn error_policy(error: &Error, _ctx: Context<ContextData>) -> Action {
    tracing::warn!("reconciler failed: {}", error);
    Action::await_change()
}

fn make_common_labels(name: &str) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("app.kubernetes.io/name".to_owned(), name.to_owned()),
        (
            "app.kubernetes.io/managed-by".to_owned(),
            PROJECT_NAME.to_owned(),
        ),
    ])
}

fn to_owner_reference(eph: &Ephemeron) -> OwnerReference {
    OwnerReference {
        api_version: Ephemeron::api_version(&()).into_owned(),
        kind: Ephemeron::kind(&()).into_owned(),
        name: eph.name(),
        uid: eph.metadata.uid.clone().expect(".metadata.uid"),
        controller: Some(true),
        block_owner_deletion: Some(true),
    }
}
