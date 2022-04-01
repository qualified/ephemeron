use k8s_openapi::api::networking::v1::{
    HTTPIngressPath, HTTPIngressRuleValue, Ingress, IngressBackend, IngressRule,
    IngressServiceBackend, IngressSpec, IngressTLS, ServiceBackendPort,
};
use kube::{
    api::{ObjectMeta, PostParams},
    error::ErrorResponse,
    runtime::controller::{Action, Context},
    Api, ResourceExt,
};
use thiserror::Error;

use super::ContextData;
use crate::Ephemeron;

#[derive(Debug, Error)]
pub enum Error {
    #[error("failed to create ingress: {0}")]
    CreateIngress(#[source] kube::Error),

    #[error("failed to get ingress: {0}")]
    GetIngress(#[source] kube::Error),
}
pub type Result<T, E = Error> = std::result::Result<T, E>;

#[tracing::instrument(skip(eph, ctx), level = "trace")]
pub(super) async fn reconcile(
    eph: &Ephemeron,
    ctx: Context<ContextData>,
) -> Result<Option<Action>> {
    let name = eph.name();
    let client = ctx.get_ref().client.clone();

    let ings: Api<Ingress> = Api::namespaced(client.clone(), super::NS);
    if ings
        .get_opt(&name)
        .await
        .map_err(Error::GetIngress)?
        .is_some()
    {
        Ok(None)
    } else {
        tracing::debug!("Creating Ingress");
        let ing = build_ingress(eph, ctx.get_ref().domain.as_ref());
        match ings.create(&PostParams::default(), &ing).await {
            Ok(_) => Ok(Some(Action::await_change())),

            Err(kube::Error::Api(ErrorResponse { code: 409, .. })) => {
                tracing::debug!("Ingress already exists");
                Ok(Some(Action::await_change()))
            }

            Err(err) => Err(Error::CreateIngress(err)),
        }
    }
}

fn build_ingress(eph: &Ephemeron, domain: &str) -> Ingress {
    let name = eph.name();
    let tls = eph.spec.service.tls_secret_name.clone().map(|name| {
        vec![IngressTLS {
            hosts: None,
            secret_name: Some(name),
        }]
    });
    Ingress {
        metadata: ObjectMeta {
            name: Some(name.clone()),
            namespace: Some(super::NS.into()),
            labels: Some(super::make_common_labels(&name)),
            owner_references: Some(vec![super::to_owner_reference(eph)]),
            annotations: Some(eph.spec.service.ingress_annotations.clone()),
            ..ObjectMeta::default()
        },
        spec: Some(IngressSpec {
            tls: Some(tls.unwrap_or_default()),
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
                                    number: Some(eph.spec.service.port),
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
