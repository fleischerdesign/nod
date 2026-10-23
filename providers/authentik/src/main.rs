//! `nod-provider-authentik`: Authentik groups for nod (ADR-030).
//!
//! Stage 0: observe and plan; `apply` refuses a non-empty change set. The
//! provider is generic: URL, token variable and marker are settings, and no
//! fleet name, contract or Nix path appears here.

mod api;
mod model;

use std::io::{self, BufRead, Write};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};

use api::{ApiError, Authentik};
use model::{Marker, ObservedResource, Resource};

/// Protocol major version this provider speaks.
const PROTOCOL: u32 = 1;

fn main() {
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }

        let request: Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(error) => {
                eprintln!("invalid request: {error}");
                std::process::exit(1);
            }
        };

        let response = match request.get("op").and_then(Value::as_str) {
            None => handshake(),
            Some("observe") => observe(&request),
            Some("plan") => plan(&request),
            Some("apply") => apply(&request),
            Some("adopt") => adopt(&request),
            Some(other) => {
                eprintln!("unknown operation: {other}");
                std::process::exit(1);
            }
        };

        match serde_json::to_string(&response) {
            Ok(text) => {
                if writeln!(stdout, "{text}").is_err() {
                    break;
                }
                let _ = stdout.flush();
            }
            Err(error) => {
                eprintln!("cannot serialise response: {error}");
                std::process::exit(1);
            }
        }
    }
}

/// The handshake answer.
fn handshake() -> Value {
    json!({
        "provider": "authentik",
        "version": env!("CARGO_PKG_VERSION"),
        "protocol": PROTOCOL,
        "capabilities": ["authentik.group"],
        "operations": ["adopt"],
        "ownership": "marker",
        "settings_schema": {
            "type": "object",
            "required": ["url"],
            "properties": {
                "url": {
                    "type": "string",
                    "description": "Base URL, e.g. https://auth.example.com"
                },
                "token_env": {
                    "type": "string",
                    "default": "AUTHENTIK_TOKEN",
                    "description": "Environment variable holding the API token"
                },
                "marker_key": {
                    "type": "string",
                    "default": "nod",
                    "description": "Group attribute key used as the ownership marker"
                },
                "marker_value": {
                    "type": "string",
                    "default": "owned",
                    "description": "Marker value"
                },
                "page_size": {
                    "type": "integer",
                    "default": 100,
                    "description": "API page size"
                },
                "timeout_secs": {
                    "type": "integer",
                    "default": 30,
                    "description": "HTTP timeout in seconds"
                }
            }
        }
    })
}

#[derive(Debug, Deserialize)]
struct Settings {
    url: String,
    #[serde(default = "default_token_env")]
    token_env: String,
    #[serde(default = "default_marker_key")]
    marker_key: String,
    #[serde(default = "default_marker_value")]
    marker_value: String,
    #[serde(default = "default_page_size")]
    page_size: u32,
    #[serde(default = "default_timeout_secs")]
    timeout_secs: u64,
}

fn default_token_env() -> String {
    "AUTHENTIK_TOKEN".to_string()
}

fn default_marker_key() -> String {
    "nod".to_string()
}

fn default_marker_value() -> String {
    "owned".to_string()
}

fn default_page_size() -> u32 {
    100
}

fn default_timeout_secs() -> u64 {
    30
}

/// An error answer, per the protocol.
fn error_message(kind: &str, message: &str) -> Value {
    json!({ "error": { "kind": kind, "message": message } })
}

/// Maps an API error to an error answer.
fn api_error(error: &ApiError) -> Value {
    json!({
        "error": {
            "kind": error.kind(),
            "message": error.message(),
            "retry_after_secs": error.retry_after(),
        }
    })
}

fn settings(request: &Value) -> Result<Settings, String> {
    let config = request.get("config").cloned().unwrap_or_else(|| json!({}));
    serde_json::from_value(config).map_err(|error| format!("invalid settings: {error}"))
}

fn client(settings: &Settings) -> Result<Authentik, String> {
    let token = std::env::var(&settings.token_env)
        .map_err(|_| format!("missing environment variable {}", settings.token_env))?;
    if token.is_empty() {
        return Err(format!(
            "environment variable {} is empty",
            settings.token_env
        ));
    }

    Ok(Authentik::new(
        &settings.url,
        token,
        settings.page_size,
        Duration::from_secs(settings.timeout_secs.max(1)),
    ))
}

fn observe(request: &Value) -> Value {
    let settings = match settings(request) {
        Ok(settings) => settings,
        Err(message) => return error_message("config", &message),
    };
    let client = match client(&settings) {
        Ok(client) => client,
        Err(message) => return error_message("config", &message),
    };
    let groups = match client.groups() {
        Ok(groups) => groups,
        Err(error) => return api_error(&error),
    };

    let marker = Marker {
        key: settings.marker_key,
        value: settings.marker_value,
    };
    let resources: Vec<Value> = groups
        .iter()
        .map(|group| {
            let resource = model::to_resource(group, &marker);
            json!({
                "kind": resource.kind,
                "id": resource.id,
                "attributes": resource.attributes,
                "owned": resource.owned,
            })
        })
        .collect();

    json!({ "resources": resources, "conditions": [] })
}

fn plan(request: &Value) -> Value {
    let desired = match request.get("desired").cloned() {
        Some(value) => match serde_json::from_value::<Vec<Resource>>(value) {
            Ok(desired) => desired,
            Err(error) => {
                eprintln!("invalid desired state: {error}");
                return error_message("protocol", "invalid desired state");
            }
        },
        None => Vec::new(),
    };
    let observed = match request.get("observed").cloned() {
        Some(value) => match serde_json::from_value::<Vec<ObservedResource>>(value) {
            Ok(observed) => observed,
            Err(error) => {
                eprintln!("invalid observed state: {error}");
                return error_message("protocol", "invalid observed state");
            }
        },
        None => Vec::new(),
    };

    json!({
        "changes": model::plan(&desired, &observed),
        "conditions": [],
    })
}

fn apply(request: &Value) -> Value {
    let changes = request
        .get("changes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    if changes.is_empty() {
        return json!({ "applied": [], "conditions": [] });
    }

    // Stage 0 is read-only: every change is refused, and the core keeps
    // `apply = false`. A silent success would be worse than a loud refusal.
    let applied: Vec<Value> = changes
        .iter()
        .map(|change| {
            let id = change
                .get("resource")
                .and_then(|resource| resource.get("id"))
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            json!({
                "id": id,
                "ok": false,
                "message": "apply is not enabled in this read-only build",
                "error": {
                    "kind": "config",
                    "message": "apply is not enabled in this read-only build"
                }
            })
        })
        .collect();

    json!({ "applied": applied, "conditions": [] })
}

fn adopt(request: &Value) -> Value {
    let resources = match request.get("resources").cloned() {
        Some(value) => match serde_json::from_value::<Vec<ObservedResource>>(value) {
            Ok(resources) => resources,
            Err(error) => {
                eprintln!("invalid adopt resources: {error}");
                return error_message("protocol", "invalid adopt resources");
            }
        },
        None => Vec::new(),
    };

    if resources.is_empty() {
        return json!({ "adopted": [], "conditions": [] });
    }

    let settings = match settings(request) {
        Ok(settings) => settings,
        Err(message) => return error_message("config", &message),
    };
    let client = match client(&settings) {
        Ok(client) => client,
        Err(message) => return error_message("config", &message),
    };
    let groups = match client.groups() {
        Ok(groups) => groups,
        Err(error) => return api_error(&error),
    };

    let marker = Marker {
        key: settings.marker_key,
        value: settings.marker_value,
    };
    let mut adopted = Vec::new();

    for resource in resources
        .iter()
        .filter(|resource| resource.kind == model::KIND_GROUP)
    {
        let Some(group) = groups.iter().find(|group| group.name == resource.id) else {
            adopted.push(json!({
                "id": resource.id,
                "ok": false,
                "message": "group not found",
                "error": { "kind": "config", "message": "group not found" }
            }));
            continue;
        };

        let attributes = marker.stamp(&group.attributes);
        match client.patch_group(&group.pk, &attributes) {
            Ok(()) => adopted.push(json!({
                "id": resource.id,
                "ok": true,
                "message": ""
            })),
            Err(error) => adopted.push(json!({
                "id": resource.id,
                "ok": false,
                "message": error.message(),
                "error": {
                    "kind": error.kind(),
                    "message": error.message(),
                    "retry_after_secs": error.retry_after()
                }
            })),
        }
    }

    json!({ "adopted": adopted, "conditions": [] })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_handshake_declares_the_group_capability() {
        let handshake = handshake();
        assert_eq!(handshake["capabilities"][0], json!("authentik.group"));
        assert_eq!(handshake["ownership"], json!("marker"));
        assert_eq!(handshake["operations"][0], json!("adopt"));
    }

    #[test]
    fn an_empty_adopt_is_a_no_op() {
        let response = adopt(&json!({ "resources": [] }));
        assert!(response["adopted"].as_array().unwrap().is_empty());
    }

    #[test]
    fn an_empty_apply_is_a_no_op() {
        let response = apply(&json!({ "changes": [] }));
        assert!(response["applied"].as_array().unwrap().is_empty());
    }

    #[test]
    fn a_non_empty_apply_is_refused() {
        let response = apply(&json!({
            "changes": [{
                "resource": { "kind": "authentik.group", "id": "admins", "attributes": {} }
            }]
        }));
        let applied = response["applied"].as_array().unwrap();
        assert_eq!(applied.len(), 1);
        assert_eq!(applied[0]["ok"], json!(false));
        assert!(applied[0]["message"]
            .as_str()
            .unwrap()
            .contains("read-only"));
    }

    #[test]
    fn a_plan_is_computed_from_desired_and_observed() {
        let response = plan(&json!({
            "desired": [{
                "kind": "authentik.group",
                "id": "admins",
                "attributes": { "is_superuser": false }
            }],
            "observed": []
        }));
        assert_eq!(response["changes"][0]["kind"], json!("create"));
    }
}
