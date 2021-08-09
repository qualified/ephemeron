use k8s_openapi::{
    api::core::v1::{Container, ContainerPort, HTTPGetAction, Pod, PodSpec, Probe},
    apimachinery::pkg::util::intstr::IntOrString,
};
use kube::{
    api::{ObjectMeta, PostParams},
    error::ErrorResponse,
    Api, ResourceExt,
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

#[tracing::instrument(skip(eph, ctx), level = "trace")]
pub(super) async fn reconcile(
    eph: &Ephemeron,
    ctx: Context<ContextData>,
) -> Result<Option<ReconcilerAction>> {
    let name = eph.name();
    let client = ctx.get_ref().client.clone();

    let pods: Api<Pod> = Api::namespaced(client.clone(), super::NS);
    match pods.get(&name).await {
        Ok(pod) => match (eph.is_pod_ready(), pod_is_ready(&pod)) {
            (a, b) if a == b => Ok(None),
            (_, actual) => {
                conditions::set_pod_ready(eph, ctx.get_ref().client.clone(), Some(actual))
                    .await
                    .context(UpdateCondition)?;
                Ok(Some(ReconcilerAction {
                    requeue_after: None,
                }))
            }
        },

        Err(kube::Error::Api(ErrorResponse { code: 404, .. })) => {
            conditions::set_pod_ready(eph, client.clone(), Some(false))
                .await
                .context(UpdateCondition)?;
            conditions::set_available(eph, client.clone(), Some(false))
                .await
                .context(UpdateCondition)?;
            let pod = build_pod(eph);
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
    let name = eph.name();
    Pod {
        metadata: ObjectMeta {
            name: Some(name.clone()),
            namespace: Some(super::NS.into()),
            owner_references: vec![super::to_owner_reference(eph)],
            labels: super::make_common_labels(&name),
            ..ObjectMeta::default()
        },
        spec: Some(PodSpec {
            containers: vec![Container {
                name: "container".into(),
                image: Some(eph.spec.service.image.clone()),
                // Note that `command` in Kubernetes corresponds to `Entrypoint` in Docker, and
                // `args` corresponds to `Cmd` in Docker.
                // See https://kubernetes.io/docs/tasks/inject-data-application/define-command-argument-container/#notes
                //
                // If `command` is specified without `args`, only the supplied `command` is used.
                // The default `Entrypoint` and `Cmd` are ignored.
                // If `command` is not specified, the default `EntryPoint` and `Cmd` are used.
                command: eph.spec.service.command.clone().unwrap_or_default(),
                working_dir: eph.spec.service.working_dir.clone(),
                ports: vec![ContainerPort {
                    container_port: eph.spec.service.port,
                    ..ContainerPort::default()
                }],
                readiness_probe: eph
                    .spec
                    .service
                    .readiness_probe
                    .as_ref()
                    .map(|probe| Probe {
                        http_get: Some(HTTPGetAction {
                            path: Some(probe.path.clone()),
                            port: IntOrString::Int(eph.spec.service.port),
                            ..HTTPGetAction::default()
                        }),
                        initial_delay_seconds: probe.initial_delay_seconds,
                        period_seconds: probe.period_seconds,
                        timeout_seconds: probe.timeout_seconds,
                        ..Probe::default()
                    }),
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
    pod.status.as_ref().map_or(false, |status| {
        status
            .conditions
            .iter()
            .any(|c| c.type_ == "Ready" && c.status == "True")
    })
}
