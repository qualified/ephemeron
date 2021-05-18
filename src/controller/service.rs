use std::collections::BTreeMap;

use k8s_openapi::{
    api::core::v1::{Service, ServicePort, ServiceSpec},
    apimachinery::pkg::util::intstr::IntOrString,
};
use kube::{
    api::{ObjectMeta, PostParams, ResourceExt},
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
    #[snafu(display("Failed to create service: {}", source))]
    CreateService { source: kube::Error },

    #[snafu(display("Failed to get service: {}", source))]
    GetService { source: kube::Error },

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

    let svcs: Api<Service> = Api::namespaced(client.clone(), super::NS);
    match svcs.get(&name).await {
        Ok(_) => Ok(None),
        Err(kube::Error::Api(ErrorResponse { code: 404, .. })) => {
            debug!("Creating Service");
            let svc = build_service(&eph);
            match svcs.create(&PostParams::default(), &svc).await {
                Ok(_) => Ok(Some(ReconcilerAction {
                    requeue_after: None,
                })),
                Err(kube::Error::Api(ErrorResponse { code: 409, .. })) => {
                    debug!("Service already exists");
                    Ok(Some(ReconcilerAction {
                        requeue_after: None,
                    }))
                }
                Err(err) => Err(Error::CreateService { source: err }),
            }
        }

        // Unexpected error
        Err(e) => Err(Error::GetService { source: e }),
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
