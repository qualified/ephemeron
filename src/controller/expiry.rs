use chrono::Utc;
use kube::{
    api::{DeleteParams, Meta, PropagationPolicy},
    Api,
};
use kube_runtime::controller::{Context, ReconcilerAction};
use snafu::{ResultExt, Snafu};
use tracing::debug;

use super::ContextData;
use crate::Ephemeron;

#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display("Failed to delete ephemeron: {}", source))]
    Delete { source: kube::Error },
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Delete the resource if it's expired.
pub(super) async fn delete_expired(
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

    Ok(Some(ReconcilerAction {
        requeue_after: None,
    }))
}
