use k8s_openapi::{
    api::core::v1::{Container, ContainerPort, EnvVar, HTTPGetAction, Pod, PodSpec, Probe},
    apimachinery::pkg::util::intstr::IntOrString,
};
use kube::{
    api::{ObjectMeta, PostParams},
    error::ErrorResponse,
    runtime::controller::{Action, Context},
    Api, ResourceExt,
};
use thiserror::Error;

use super::{conditions, ContextData};
use crate::Ephemeron;

#[derive(Debug, Error)]
pub enum Error {
    #[error("failed to create pod: {0}")]
    CreatePod(#[source] kube::Error),

    #[error("failed to get god: {0}")]
    GetPod(#[source] kube::Error),

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

    let pods: Api<Pod> = Api::namespaced(client.clone(), super::NS);
    if let Some(pod) = pods.get_opt(&name).await.map_err(Error::GetPod)? {
        match (eph.is_pod_ready(), pod_is_ready(&pod)) {
            (a, b) if a == b => Ok(None),
            (_, actual) => {
                conditions::set_pod_ready(eph, ctx.get_ref().client.clone(), Some(actual))
                    .await
                    .map_err(Error::UpdateCondition)?;
                Ok(Some(Action::await_change()))
            }
        }
    } else {
        conditions::set_pod_ready(eph, client.clone(), Some(false))
            .await
            .map_err(Error::UpdateCondition)?;
        conditions::set_available(eph, client.clone(), Some(false))
            .await
            .map_err(Error::UpdateCondition)?;
        let pod = build_pod(eph);
        match pods.create(&PostParams::default(), &pod).await {
            Ok(_) => Ok(Some(Action::await_change())),
            Err(kube::Error::Api(ErrorResponse { code: 409, .. })) => {
                tracing::debug!("Pod already exists");
                Ok(Some(Action::await_change()))
            }
            Err(err) => Err(Error::CreatePod(err)),
        }
    }
}

fn build_pod(eph: &Ephemeron) -> Pod {
    let name = eph.name();
    let mut labels = eph.spec.service.pod_labels.clone();
    labels.append(&mut super::make_common_labels(&name));
    Pod {
        metadata: ObjectMeta {
            name: Some(name),
            namespace: Some(super::NS.into()),
            owner_references: Some(vec![super::to_owner_reference(eph)]),
            labels: Some(labels),
            ..ObjectMeta::default()
        },
        spec: Some(PodSpec {
            containers: vec![Container {
                name: "container".into(),
                image: Some(eph.spec.service.image.clone()),
                image_pull_policy: eph.spec.service.image_pull_policy.clone(),
                // Note that `command` in Kubernetes corresponds to `Entrypoint` in Docker, and
                // `args` corresponds to `Cmd` in Docker.
                // See https://kubernetes.io/docs/tasks/inject-data-application/define-command-argument-container/#notes
                //
                // If `command` is specified without `args`, only the supplied `command` is used.
                // The default `Entrypoint` and `Cmd` are ignored.
                // If `command` is not specified, the default `EntryPoint` and `Cmd` are used.
                command: Some(eph.spec.service.command.clone().unwrap_or_default()),
                env: eph.spec.service.env.clone().map(|v| {
                    v.into_iter()
                        .map(|e| EnvVar {
                            name: e.name,
                            value: e.value,
                            value_from: None,
                        })
                        .collect()
                }),
                working_dir: eph.spec.service.working_dir.clone(),
                ports: Some(vec![ContainerPort {
                    container_port: eph.spec.service.port,
                    ..ContainerPort::default()
                }]),
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
                resources: eph.spec.service.resources.clone(),
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
