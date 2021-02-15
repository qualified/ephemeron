use std::{collections::BTreeMap, time::Duration};

use chrono::Utc;
use futures::StreamExt;
use k8s_openapi::{
    api::{
        core::v1::{
            Container, ContainerPort, Endpoints, Pod, PodSpec, Service, ServicePort, ServiceSpec,
        },
        networking::v1::{
            HTTPIngressPath, HTTPIngressRuleValue, Ingress, IngressBackend, IngressRule,
            IngressServiceBackend, IngressSpec, ServiceBackendPort,
        },
    },
    apimachinery::pkg::{apis::meta::v1::OwnerReference, util::intstr::IntOrString},
    Resource,
};
use kube::{
    api::{
        DeleteParams, ListParams, Meta, ObjectMeta, Patch, PatchParams, PostParams,
        PropagationPolicy,
    },
    error::ErrorResponse,
    Api, Client,
};
use kube_runtime::controller::{Context, Controller, ReconcilerAction};
use snafu::{ResultExt, Snafu};
use tracing::{debug, trace, warn};

use super::{Ephemeron, EphemeronCondition, EphemeronStatus};

const PROJECT_NAME: &str = "ephemeron";
#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display("Failed to create pod: {}", source))]
    CreatePod { source: kube::Error },

    #[snafu(display("Failed to create service: {}", source))]
    CreateService { source: kube::Error },

    #[snafu(display("Failed to create ingress: {}", source))]
    CreateIngress { source: kube::Error },

    #[snafu(display("Failed to get god: {}", source))]
    GetPod { source: kube::Error },

    #[snafu(display("Failed to get service: {}", source))]
    GetService { source: kube::Error },

    #[snafu(display("Failed to get ingress: {}", source))]
    GetIngress { source: kube::Error },

    #[snafu(display("Failed to get endpoints: {}", source))]
    GetEndpoints { source: kube::Error },

    #[snafu(display("Failed to delete ephemeron: {}", source))]
    Delete { source: kube::Error },

    #[snafu(display("Failed to update ephemeron status: {}", source))]
    UpdateStatus { source: kube::Error },

    #[snafu(display("Failed to annotate host information: {}", source))]
    HostAnnotation { source: kube::Error },
}
pub type Result<T, E = Error> = std::result::Result<T, E>;

// TODO Configurable
const NS: &str = "default";

pub async fn run(client: Client, domain: String) {
    let lp =
        ListParams::default().labels(&format!("app.kubernetes.io/managed-by={}", PROJECT_NAME));
    let controller = Controller::<Ephemeron>::new(Api::all(client.clone()), ListParams::default())
        .owns::<Pod>(Api::namespaced(client.clone(), NS), lp.clone())
        .owns::<Service>(Api::namespaced(client.clone(), NS), lp.clone())
        .owns::<Ingress>(Api::namespaced(client.clone(), NS), lp);

    let context = Context::new(ContextData {
        client: client.clone(),
        domain,
    });

    controller
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
    let name = Meta::name(&eph);
    match eph.status.as_ref() {
        None => {
            debug!("First reconciliation");
            debug!("Patching status");
            let client = ctx.get_ref().client.clone();
            set_pod_ready(&eph, client.clone(), None).await?;
            set_available(&eph, client, None).await?;

            Ok(ReconcilerAction {
                requeue_after: None,
            })
        }

        Some(status) => {
            trace!("conditions: {:?}", status.conditions);

            // Expired condition must be checked first.
            if eph.spec.expires <= Utc::now() {
                debug!("Resource expired, deleting");
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

                return Ok(ReconcilerAction {
                    requeue_after: None,
                });
            }

            let client = ctx.get_ref().client.clone();
            // TODO Get or Create (ignore 409 conflict)
            // Create pod if missing.
            let pods: Api<Pod> = Api::namespaced(client.clone(), NS);
            match pods.get(&name).await {
                Ok(pod) => {
                    if !eph.is_pod_ready() && pod_is_ready(&pod) {
                        set_pod_ready(&eph, ctx.get_ref().client.clone(), Some(true)).await?;
                        return Ok(ReconcilerAction {
                            requeue_after: None,
                        });
                    }
                }

                Err(kube::Error::Api(ErrorResponse { code: 404, .. })) => {
                    set_pod_ready(&eph, client.clone(), Some(false)).await?;
                    set_available(&eph, client.clone(), Some(false)).await?;
                    let pod = build_pod(&eph);
                    return match pods.create(&PostParams::default(), &pod).await {
                        Ok(_) => Ok(ReconcilerAction {
                            requeue_after: None,
                        }),
                        Err(kube::Error::Api(ErrorResponse { code: 409, .. })) => {
                            debug!("Pod already exists");
                            Ok(ReconcilerAction {
                                requeue_after: None,
                            })
                        }
                        Err(err) => Err(Error::CreatePod { source: err }),
                    };
                }
                // Unexpected error
                Err(e) => return Err(Error::GetPod { source: e }),
            }

            // Create Service if missing.
            let svcs: Api<Service> = Api::namespaced(client.clone(), NS);
            match svcs.get(&name).await {
                Ok(_) => {}
                Err(kube::Error::Api(ErrorResponse { code: 404, .. })) => {
                    debug!("Creating Service");
                    let svc = build_service(&eph);
                    return match svcs.create(&PostParams::default(), &svc).await {
                        Ok(_) => Ok(ReconcilerAction {
                            requeue_after: None,
                        }),
                        Err(kube::Error::Api(ErrorResponse { code: 409, .. })) => {
                            debug!("Service already exists");
                            Ok(ReconcilerAction {
                                requeue_after: None,
                            })
                        }
                        Err(err) => Err(Error::CreateService { source: err }),
                    };
                }
                // Unexpected error
                Err(e) => return Err(Error::GetService { source: e }),
            }

            // Create Ingress if missing.
            let ings: Api<Ingress> = Api::namespaced(client.clone(), NS);
            match ings.get(&name).await {
                Ok(_) => {}
                Err(kube::Error::Api(ErrorResponse { code: 404, .. })) => {
                    debug!("Creating Ingress");
                    let ing = build_ingress(&eph, ctx.get_ref().domain.as_ref());
                    return match ings.create(&PostParams::default(), &ing).await {
                        Ok(_) => Ok(ReconcilerAction {
                            requeue_after: None,
                        }),
                        Err(kube::Error::Api(ErrorResponse { code: 409, .. })) => {
                            debug!("Ingress already exists");
                            Ok(ReconcilerAction {
                                requeue_after: None,
                            })
                        }
                        Err(err) => Err(Error::CreateIngress { source: err }),
                    };
                }
                // Unexpected error
                Err(e) => return Err(Error::GetIngress { source: e }),
            }

            if !eph.is_available() {
                // Check if service has endpoints
                let eps: Api<Endpoints> = Api::namespaced(client.clone(), NS);
                return match eps.get(&name).await {
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

                        set_available(&eph, client, Some(true)).await?;
                        Ok(ReconcilerAction {
                            requeue_after: None,
                        })
                    }

                    Ok(_) | Err(kube::Error::Api(ErrorResponse { code: 404, .. })) => {
                        Ok(ReconcilerAction {
                            requeue_after: Some(Duration::from_secs(1)),
                        })
                    }

                    Err(err) => return Err(Error::GetEndpoints { source: err }),
                };
            }

            // If children are there, requeue after some time.
            debug!("Requeue later");
            Ok(ReconcilerAction {
                requeue_after: Some((eph.spec.expires - Utc::now()).to_std().unwrap_or_default()),
            })
        }
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

#[tracing::instrument(skip(eph, client), level = "debug")]
async fn set_pod_ready(eph: &Ephemeron, client: Client, status: Option<bool>) -> Result<()> {
    set_condition(eph, client, EphemeronCondition::pod_ready(status)).await
}

#[tracing::instrument(skip(eph, client), level = "debug")]
async fn set_available(eph: &Ephemeron, client: Client, status: Option<bool>) -> Result<()> {
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
    let name = Meta::name(eph);
    let api: Api<Ephemeron> = Api::all(client);
    api.patch_status(
        &name,
        &ssapply,
        &Patch::Apply(serde_json::json!({
            "apiVersion": <Ephemeron as Resource>::API_VERSION,
            "kind": <Ephemeron as Resource>::KIND,
            "status": EphemeronStatus {
                conditions: vec![condition],
                observed_generation: eph.metadata.generation,
            },
        })),
    )
    .await
    .context(UpdateStatus)?;

    Ok(())
}

fn build_pod(eph: &Ephemeron) -> Pod {
    let name = Meta::name(eph);
    Pod {
        metadata: ObjectMeta {
            name: Some(name.clone()),
            namespace: Some(NS.into()),
            owner_references: Some(vec![to_owner_reference(eph)]),
            labels: Some(make_common_labels(&name)),
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

fn build_service(eph: &Ephemeron) -> Service {
    let name = Meta::name(eph);
    Service {
        metadata: ObjectMeta {
            name: Some(name.clone()),
            namespace: Some(NS.into()),
            owner_references: Some(vec![to_owner_reference(eph)]),
            labels: Some(make_common_labels(&name)),
            ..ObjectMeta::default()
        },
        spec: Some(ServiceSpec {
            type_: Some("ClusterIP".into()),
            ports: Some(vec![ServicePort {
                port: eph.spec.port,
                target_port: Some(IntOrString::Int(eph.spec.port)),
                ..ServicePort::default()
            }]),
            selector: Some(
                vec![("app.kubernetes.io/name".to_owned(), name)]
                    .into_iter()
                    .collect::<BTreeMap<_, _>>(),
            ),
            ..ServiceSpec::default()
        }),
        ..Service::default()
    }
}

fn build_ingress(eph: &Ephemeron, domain: &str) -> Ingress {
    let name = Meta::name(eph);
    Ingress {
        metadata: ObjectMeta {
            name: Some(name.clone()),
            namespace: Some(NS.into()),
            labels: Some(make_common_labels(&name)),
            owner_references: Some(vec![to_owner_reference(eph)]),
            ..ObjectMeta::default()
        },
        spec: Some(IngressSpec {
            rules: Some(vec![IngressRule {
                host: Some(format!("{}.{}", name, domain)),
                http: Some(HTTPIngressRuleValue {
                    paths: vec![HTTPIngressPath {
                        path: Some("/".into()),
                        path_type: Some("Prefix".into()),
                        backend: IngressBackend {
                            service: Some(IngressServiceBackend {
                                name: name.clone(),
                                port: Some(ServiceBackendPort {
                                    number: Some(eph.spec.port),
                                    name: None,
                                }),
                            }),
                            resource: None,
                        },
                    }],
                }),
            }]),
            ..IngressSpec::default()
        }),
        ..Ingress::default()
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

fn pod_is_ready(pod: &Pod) -> bool {
    pod.status
        .as_ref()
        .and_then(|s| s.conditions.as_ref())
        .map_or(false, |cs| {
            cs.iter().any(|c| c.type_ == "Ready" && c.status == "True")
        })
}
