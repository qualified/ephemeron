use chrono::Utc;
use kube::{
    api::{DeleteParams, PropagationPolicy},
    runtime::controller::{Action, Context},
    Api, ResourceExt,
};
use thiserror::Error;
use tracing::debug;

use super::ContextData;
use crate::Ephemeron;

#[derive(Debug, Error)]
pub enum Error {
    #[error("failed to delete ephemeron: {0}")]
    Delete(#[source] kube::Error),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Delete the resource if it's expired.
#[tracing::instrument(skip(eph, ctx), level = "trace")]
pub(super) async fn reconcile(
    eph: &Ephemeron,
    ctx: Context<ContextData>,
) -> Result<Option<Action>> {
    if eph.spec.expiration_time > Utc::now() {
        return Ok(None);
    }

    debug!("Resource expired, deleting");
    let name = eph.name();
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
    .map_err(Error::Delete)?;

    Ok(Some(Action::await_change()))
}
