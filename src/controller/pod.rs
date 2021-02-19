use k8s_openapi::api::core::v1::{Container, ContainerPort, Pod, PodSpec};
use kube::{
    api::{Meta, ObjectMeta, PostParams},
    error::ErrorResponse,
    Api,
};
use kube_runtime::controller::{Context, ReconcilerAction};
use snafu::{ResultExt, Snafu};
use tracing::debug;

use super::{conditions, ContextData};
use crate::Ephemeron;

#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display("Failed to create pod: {}", source))]
    CreatePod { source: kube::Error },

    #[snafu(display("Failed to get god: {}", source))]
    GetPod { source: kube::Error },

    #[snafu(display("Failed to update condition: {}", source))]
    UpdateCondition { source: conditions::Error },
}
pub type Result<T, E = Error> = std::result::Result<T, E>;

pub(super) async fn reconcile(
    eph: &Ephemeron,
    ctx: Context<ContextData>,
) -> Result<Option<ReconcilerAction>> {
    let name = Meta::name(eph);
    let client = ctx.get_ref().client.clone();

    let pods: Api<Pod> = Api::namespaced(client.clone(), super::NS);
    match pods.get(&name).await {
        Ok(pod) => {
            if !eph.is_pod_ready() && pod_is_ready(&pod) {
                conditions::set_pod_ready(&eph, ctx.get_ref().client.clone(), Some(true))
                    .await
                    .context(UpdateCondition)?;
                Ok(Some(ReconcilerAction {
                    requeue_after: None,
                }))
            } else {
                Ok(None)
            }
        }

        Err(kube::Error::Api(ErrorResponse { code: 404, .. })) => {
            conditions::set_pod_ready(&eph, client.clone(), Some(false))
                .await
                .context(UpdateCondition)?;
            conditions::set_available(&eph, client.clone(), Some(false))
                .await
                .context(UpdateCondition)?;
            let pod = build_pod(&eph);
            match pods.create(&PostParams::default(), &pod).await {
                Ok(_) => Ok(Some(ReconcilerAction {
                    requeue_after: None,
                })),
                Err(kube::Error::Api(ErrorResponse { code: 409, .. })) => {
                    debug!("Pod already exists");
                    Ok(Some(ReconcilerAction {
                        requeue_after: None,
                    }))
                }
                Err(err) => Err(Error::CreatePod { source: err }),
            }
        }

        // Unexpected error
        Err(e) => Err(Error::GetPod { source: e }),
    }
}

fn build_pod(eph: &Ephemeron) -> Pod {
    let name = Meta::name(eph);
    Pod {
        metadata: ObjectMeta {
            name: Some(name.clone()),
            namespace: Some(super::NS.into()),
            owner_references: Some(vec![super::to_owner_reference(eph)]),
            labels: Some(super::make_common_labels(&name)),
            ..ObjectMeta::default()
        },
        spec: Some(PodSpec {
            containers: vec![Container {
                name: "container".into(),
                image: Some(eph.spec.image.clone()),
                command: eph.spec.command.clone(),
                ports: Some(vec![ContainerPort {
                    container_port: eph.spec.port,
                    ..ContainerPort::default()
                }]),
                ..Container::default()
            }],
            restart_policy: Some("Always".into()),
            // Don't inject information about services.
            enable_service_links: Some(false),
            ..PodSpec::default()
        }),
        ..Pod::default()
    }
}

fn pod_is_ready(pod: &Pod) -> bool {
    pod.status
        .as_ref()
        .and_then(|s| s.conditions.as_ref())
        .map_or(false, |cs| {
            cs.iter().any(|c| c.type_ == "Ready" && c.status == "True")
        })
}
