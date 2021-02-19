use k8s_openapi::api::networking::v1::{
    HTTPIngressPath, HTTPIngressRuleValue, Ingress, IngressBackend, IngressRule,
    IngressServiceBackend, IngressSpec, ServiceBackendPort,
};
use kube::{
    api::{Meta, ObjectMeta, PostParams},
    error::ErrorResponse,
    Api,
};
use kube_runtime::controller::{Context, ReconcilerAction};
use snafu::Snafu;
use tracing::debug;

use super::{conditions, ContextData};
use crate::Ephemeron;

#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display("Failed to create ingress: {}", source))]
    CreateIngress { source: kube::Error },

    #[snafu(display("Failed to get ingress: {}", source))]
    GetIngress { source: kube::Error },

    #[snafu(display("Failed to get endpoints: {}", source))]
    GetEndpoints { source: kube::Error },

    #[snafu(display("Failed to delete ephemeron: {}", source))]
    Delete { source: kube::Error },

    #[snafu(display("Failed to annotate host information: {}", source))]
    HostAnnotation { source: kube::Error },

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

    let ings: Api<Ingress> = Api::namespaced(client.clone(), super::NS);
    match ings.get(&name).await {
        Ok(_) => Ok(None),

        Err(kube::Error::Api(ErrorResponse { code: 404, .. })) => {
            debug!("Creating Ingress");
            let ing = build_ingress(&eph, ctx.get_ref().domain.as_ref());
            match ings.create(&PostParams::default(), &ing).await {
                Ok(_) => Ok(Some(ReconcilerAction {
                    requeue_after: None,
                })),

                Err(kube::Error::Api(ErrorResponse { code: 409, .. })) => {
                    debug!("Ingress already exists");
                    Ok(Some(ReconcilerAction {
                        requeue_after: None,
                    }))
                }

                Err(err) => Err(Error::CreateIngress { source: err }),
            }
        }

        // Unexpected error
        Err(e) => Err(Error::GetIngress { source: e }),
    }
}

fn build_ingress(eph: &Ephemeron, domain: &str) -> Ingress {
    let name = Meta::name(eph);
    Ingress {
        metadata: ObjectMeta {
            name: Some(name.clone()),
            namespace: Some(super::NS.into()),
            labels: Some(super::make_common_labels(&name)),
            owner_references: Some(vec![super::to_owner_reference(eph)]),
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