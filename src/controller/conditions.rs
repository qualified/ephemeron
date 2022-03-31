use kube::{
    api::{Patch, PatchParams},
    Api, Client, Resource, ResourceExt,
};
use thiserror::Error;

use crate::{Ephemeron, EphemeronCondition, EphemeronStatus};

#[derive(Debug, Error)]
pub enum Error {
    #[error("failed to update ephemeron status: {0}")]
    UpdateStatus(#[source] kube::Error),
}
pub type Result<T, E = Error> = std::result::Result<T, E>;

#[tracing::instrument(skip(eph, client), level = "debug")]
pub async fn set_pod_ready(eph: &Ephemeron, client: Client, status: Option<bool>) -> Result<()> {
    set_condition(eph, client, EphemeronCondition::pod_ready(status)).await
}

#[tracing::instrument(skip(eph, client), level = "debug")]
pub async fn set_available(eph: &Ephemeron, client: Client, status: Option<bool>) -> Result<()> {
    set_condition(eph, client, EphemeronCondition::available(status)).await
}

async fn set_condition(
    eph: &Ephemeron,
    client: Client,
    condition: EphemeronCondition,
) -> Result<()> {
    // > It is strongly recommended for controllers to always "force" conflicts,
    // > since they might not be able to resolve or act on these conflicts.
    // > https://kubernetes.io/docs/reference/using-api/server-side-apply/#using-server-side-apply-in-a-controller
    let ssapply = PatchParams::apply(condition.manager()).force();
    let name = eph.name();
    let api: Api<Ephemeron> = Api::all(client);
    api.patch_status(
        &name,
        &ssapply,
        &Patch::Apply(serde_json::json!({
            "apiVersion": Ephemeron::api_version(&()),
            "kind": Ephemeron::kind(&()),
            "status": EphemeronStatus {
                conditions: vec![condition],
                observed_generation: eph.metadata.generation,
            },
        })),
    )
    .await
    .map_err(Error::UpdateStatus)?;

    Ok(())
}
