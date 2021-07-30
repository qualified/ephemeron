// https://github.com/GREsau/schemars/pull/65
#![allow(clippy::field_reassign_with_default)]
// From `CustomResource`
#![allow(clippy::default_trait_access)]

use chrono::{DateTime, Utc};
use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

mod schemas;

#[derive(CustomResource, Deserialize, Serialize, Debug, PartialEq, Clone, JsonSchema)]
#[kube(
    group = "qualified.io",
    version = "v1alpha1",
    kind = "Ephemeron",
    plural = "ephemerons",
    shortname = "eph",
    shortname = "ephs",
    status = "EphemeronStatus"
)]
#[serde(rename_all = "camelCase")]
pub struct EphemeronSpec {
    /// The date and time to kill this service on.
    pub expires: DateTime<Utc>,
    /// The service to create.
    pub service: EphemeronService,
}

#[derive(Deserialize, Serialize, Debug, PartialEq, Clone, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EphemeronService {
    /// The image to use.
    pub image: String,
    /// Optionally specify the command to use.
    pub command: Option<Vec<String>>,
    /// The directory to run command in.
    pub working_dir: Option<String>,
    /// The port to use.
    #[schemars(schema_with = "schemas::port")]
    pub port: i32,
    /// The name of the TLS secret.
    pub tls_secret_name: Option<String>,
    /// Ingress annotations.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub ingress_annotations: std::collections::BTreeMap<String, String>,
}

#[derive(Deserialize, Serialize, Debug, Clone, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EphemeronStatus {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(schema_with = "schemas::conditions")]
    pub conditions: Vec<EphemeronCondition>,

    /// The last reconciled generation.
    #[schemars(schema_with = "schemas::observed_generation")]
    pub observed_generation: Option<i64>,
}

// Helper methods for conditions.
impl Ephemeron {
    pub(crate) fn is_pod_ready(&self) -> bool {
        self.find_condition(|c| {
            matches!(
                c,
                EphemeronCondition::PodReady {
                    status: Some(true),
                    ..
                }
            )
        })
        .is_some()
    }

    pub(crate) fn is_available(&self) -> bool {
        self.find_condition(|c| {
            matches!(
                c,
                EphemeronCondition::Available {
                    status: Some(true),
                    ..
                }
            )
        })
        .is_some()
    }

    fn find_condition<F>(&self, mut f: F) -> Option<&EphemeronCondition>
    where
        F: FnMut(&EphemeronCondition) -> bool,
    {
        self.status
            .as_ref()
            .and_then(|s| s.conditions.iter().find(|&c| f(c)))
    }
}

#[derive(Deserialize, Serialize, Debug, PartialEq, Clone, JsonSchema)]
#[serde(tag = "type")]
pub enum EphemeronCondition {
    #[serde(rename_all = "camelCase")]
    PodReady {
        /// Status of the condition. Maps to String enum: Unknown, True, False.
        #[serde(
            serialize_with = "condition_status_ser",
            deserialize_with = "condition_status_de"
        )]
        status: Option<bool>,

        // TODO Use the time from Pod?
        /// Last time the condition transitioned from one status to another.
        last_transition_time: DateTime<Utc>,
    },

    #[serde(rename_all = "camelCase")]
    Available {
        /// Status of the condition. Maps to String enum: Unknown, True, False.
        #[serde(
            serialize_with = "condition_status_ser",
            deserialize_with = "condition_status_de"
        )]
        status: Option<bool>,

        /// Last time the condition transitioned from one status to another.
        last_transition_time: DateTime<Utc>,
    },
}

// The names of managers to be used to update the field in controller.
const POD_READY_MANAGER: &str = "ephemeron-podready";
const AVAILABLE_MANAGER: &str = "ephemeron-available";

impl EphemeronCondition {
    pub(crate) fn manager(&self) -> &str {
        match self {
            EphemeronCondition::PodReady { .. } => POD_READY_MANAGER,
            EphemeronCondition::Available { .. } => AVAILABLE_MANAGER,
        }
    }

    pub(crate) fn pod_ready(status: Option<bool>) -> Self {
        Self::PodReady {
            status,
            last_transition_time: Utc::now(),
        }
    }

    pub(crate) fn available(status: Option<bool>) -> Self {
        Self::Available {
            status,
            last_transition_time: Utc::now(),
        }
    }
}

fn condition_status_de<'de, D>(deserializer: D) -> Result<Option<bool>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    match String::deserialize(deserializer)?.as_ref() {
        "Unknown" => Ok(None),
        "True" => Ok(Some(true)),
        "False" => Ok(Some(false)),
        other => Err(serde::de::Error::invalid_value(
            serde::de::Unexpected::Str(other),
            &"Unknown or True or False",
        )),
    }
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn condition_status_ser<S>(status: &Option<bool>, s: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    s.serialize_str(match status {
        Some(true) => "True",
        Some(false) => "False",
        None => "Unknown",
    })
}
