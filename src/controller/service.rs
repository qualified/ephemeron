use std::collections::BTreeMap;

use k8s_openapi::{
    api::core::v1::{Service, ServicePort, ServiceSpec},
    apimachinery::pkg::util::intstr::IntOrString,
};
use kube::{
    api::{ObjectMeta, PostParams},
    error::ErrorResponse,
    runtime::controller::{Action, Context},
    Api, ResourceExt,
};
use thiserror::Error;
use tracing::debug;

use super::ContextData;
use crate::Ephemeron;

#[derive(Debug, Error)]
pub enum Error {
    #[error("failed to create service: {0}")]
    CreateService(#[source] kube::Error),

    #[error("failed to get service: {0}")]
    GetService(#[source] kube::Error),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[tracing::instrument(skip(eph, ctx), level = "trace")]
pub(super) async fn reconcile(
    eph: &Ephemeron,
    ctx: Context<ContextData>,
) -> Result<Option<Action>> {
    let name = eph.name();
    let client = ctx.get_ref().client.clone();

    let svcs: Api<Service> = Api::namespaced(client.clone(), super::NS);
    if svcs
        .get_opt(&name)
        .await
        .map_err(Error::GetService)?
        .is_some()
    {
        Ok(None)
    } else {
        debug!("Creating Service");
        let svc = build_service(eph);
        match svcs.create(&PostParams::default(), &svc).await {
            Ok(_) => Ok(Some(Action::await_change())),
            Err(kube::Error::Api(ErrorResponse { code: 409, .. })) => {
                debug!("Service already exists");
                Ok(Some(Action::await_change()))
            }
            Err(err) => Err(Error::CreateService(err)),
        }
    }
}

fn build_service(eph: &Ephemeron) -> Service {
    let name = eph.name();
    Service {
        metadata: ObjectMeta {
            name: Some(name.clone()),
            namespace: Some(super::NS.into()),
            owner_references: Some(vec![super::to_owner_reference(eph)]),
            labels: Some(super::make_common_labels(&name)),
            ..ObjectMeta::default()
        },
        spec: Some(ServiceSpec {
            type_: Some("ClusterIP".into()),
            ports: Some(vec![ServicePort {
                port: eph.spec.service.port,
                target_port: Some(IntOrString::Int(eph.spec.service.port)),
                ..ServicePort::default()
            }]),
            selector: Some(BTreeMap::from([(
                "app.kubernetes.io/name".to_owned(),
                name,
            )])),
            ..ServiceSpec::default()
        }),
        ..Service::default()
    }
}
