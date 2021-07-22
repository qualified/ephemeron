use std::collections::BTreeMap;

use serde::Deserialize;

use crate::EphemeronService;

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    #[serde(default)]
    pub presets: Presets,
}

pub type Presets = BTreeMap<String, EphemeronService>;

/// Payload for creating service with a preset.
#[derive(Deserialize, Debug, PartialEq, Clone)]
pub struct PresetPayload {
    /// The name of the preset to use.
    pub preset: String,
    /// The duration to expire the service after.
    pub duration: String,
}
