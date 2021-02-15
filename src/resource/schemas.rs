//! Custom schema functions.
use schemars::{gen::SchemaGenerator, schema::Schema};
use serde_json::{from_value, json};

pub fn port(_: &mut SchemaGenerator) -> Schema {
    from_value(json!({
        "type": "integer",
        "minimum": 1,
        "maximum": 65535,
    }))
    .unwrap()
}

pub fn observed_generation(_: &mut SchemaGenerator) -> Schema {
    from_value(json!({
        "type": "integer",
        "format": "int64",
        "minimum": 0
    }))
    .unwrap()
}

// Custom schema is necessary for `.status.conditions` because we need to add
// `x-kubernetes-list-type: map` and `x-kubernetes-list-map-keys: [type]` to update with
// server side apply.
pub fn conditions(_: &mut SchemaGenerator) -> Schema {
    from_value(json!({
        "type": "array",
        "x-kubernetes-list-type": "map",
        "x-kubernetes-list-map-keys": ["type"],
        "items": {
            "type": "object",
            "properties": {
                "lastTransitionTime": {
                    "description": "Last time the condition transitioned from one status to another.",
                    "format": "date-time",
                    "type": "string"
                },
                "status": {
                    "default": "Unknown",
                    "description": "Status of the condition.",
                    "enum": [
                        "Unknown",
                        "True",
                        "False"
                    ],
                    "type": "string"
                },
                "type": {
                    "description": "Type of condition.",
                    "pattern": "^([A-Za-z0-9][-A-Za-z0-9_.]*)?[A-Za-z0-9]$",
                    "type": "string"
                }
            },
            "required": [
                "lastTransitionTime",
                "status",
                "type"
            ],
        },
    }))
    .unwrap()
}
