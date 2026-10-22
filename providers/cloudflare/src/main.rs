//! `nod-provider-cloudflare`: the Cloudflare DNS provider for nod (ADR-030).
//!
//! Speaks JSON Lines over stdin/stdout. This package depends on no nod crate:
//! the protocol is the contract, and a provider that imports the core would be
//! a plugin, not an ecosystem.

mod cf;
mod model;

use std::io::{self, BufRead, Write};

use serde_json::{json, Value};

use cf::{CfError, Cloudflare};
use model::{Desired, Resource};

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
        "provider": "cloudflare",
        "version": env!("CARGO_PKG_VERSION"),
        "protocol": PROTOCOL,
        "capabilities": ["dns.record"],
        "operations": ["adopt"],
        "ownership": "marker",
        "settings_schema": {
            "type": "object",
            "required": ["zone"],
            "properties": {
                "zone": { "type": "string", "description": "Zone name, e.g. example.com" },
                "token_env": {
                    "type": "string",
                    "default": "CLOUDFLARE_API_TOKEN",
                    "description": "Environment variable holding the API token"
                }
            }
        }
    })
}

/// Provider settings for one request.
struct Settings {
    zone: String,
    token: String,
}

/// Reads settings from the request, or explains what is missing.
fn settings(request: &Value) -> Result<Settings, String> {
    let config = request.get("config").cloned().unwrap_or_else(|| json!({}));
    let zone = config
        .get("zone")
        .and_then(Value::as_str)
        .ok_or_else(|| "config.zone is required".to_string())?;
    let token_env = config
        .get("token_env")
        .and_then(Value::as_str)
        .unwrap_or("CLOUDFLARE_API_TOKEN");
    let token = std::env::var(token_env)
        .map_err(|_| format!("environment variable {token_env} is not set"))?;

    Ok(Settings {
        zone: zone.to_string(),
        token,
    })
}

/// An error answer.
///
/// Never an empty result: `resources: []` would read as an empty zone and turn
/// an unobserved world into a healthy one.
fn error_message(kind: &str, message: &str) -> Value {
    json!({ "error": { "kind": kind, "message": message } })
}

/// Maps an API error to an error answer.
fn api_error(error: &CfError) -> Value {
    error_message(error.condition_kind(), error.detail())
}

/// Observes every DNS record in the zone.
fn observe(request: &Value) -> Value {
    let settings = match settings(request) {
        Ok(settings) => settings,
        Err(message) => return error_message("config", &message),
    };
    let client = Cloudflare::new(settings.token);

    let zone_id = match client.zone_id(&settings.zone) {
        Ok(zone_id) => zone_id,
        Err(error) => return api_error(&error),
    };
    let records = match client.list_records(&zone_id) {
        Ok(records) => records,
        Err(error) => return api_error(&error),
    };

    let resources: Vec<Value> = records
        .iter()
        .map(|record| {
            let (resource, owned) = model::to_resource(record);
            json!({
                "kind": resource.kind,
                "id": resource.id,
                "attributes": resource.attributes,
                "owned": owned,
            })
        })
        .collect();

    json!({ "resources": resources, "conditions": [] })
}

/// Parses the observed resources the core passed back.
fn observed_resources(request: &Value) -> Vec<(Resource, bool)> {
    request
        .get("observed")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let resource: Resource = serde_json::from_value(json!({
                        "kind": item.get("kind")?,
                        "id": item.get("id")?,
                        "attributes": item.get("attributes").cloned().unwrap_or_else(|| json!({})),
                    }))
                    .ok()?;
                    let owned = item.get("owned").and_then(Value::as_bool).unwrap_or(false);
                    Some((resource, owned))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Plans by comparing the declaration with the observation.
fn plan(request: &Value) -> Value {
    let desired: Vec<Desired> = request
        .get("desired")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .unwrap_or_else(|error| {
            eprintln!("invalid desired state: {error}");
            None
        })
        .unwrap_or_default();

    let changes = model::plan(&desired, &observed_resources(request));
    json!({ "changes": changes, "conditions": [] })
}

/// Applies the changes the core approved.
fn apply(request: &Value) -> Value {
    let settings = match settings(request) {
        Ok(settings) => settings,
        Err(message) => return error_message("config", &message),
    };
    let client = Cloudflare::new(settings.token);
    let zone_id = match client.zone_id(&settings.zone) {
        Ok(zone_id) => zone_id,
        Err(error) => return api_error(&error),
    };

    let empty = Vec::new();
    let changes = request
        .get("changes")
        .and_then(Value::as_array)
        .unwrap_or(&empty);

    let mut applied = Vec::new();
    for change in changes {
        let kind = change.get("kind").and_then(Value::as_str).unwrap_or("");
        let resource = change.get("resource").cloned().unwrap_or_else(|| json!({}));
        let id = resource
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let attributes = resource
            .get("attributes")
            .cloned()
            .unwrap_or_else(|| json!({}));

        let result = match kind {
            "create" => create_record(&client, &zone_id, &attributes),
            "update" => update_record(&client, &zone_id, &attributes),
            "delete" => delete_record(&client, &zone_id, &attributes),
            other => Err(CfError::Transient(format!("unknown change kind: {other}"))),
        };

        applied.push(match result {
            Ok(()) => json!({ "id": id, "ok": true }),
            Err(error) => {
                json!({ "id": id, "ok": false, "message": error.detail() })
            }
        });
    }

    json!({ "applied": applied, "conditions": [] })
}

/// Attributes as strings/numbers, with the defaults Cloudflare expects.
fn field_str(attributes: &Value, key: &str) -> String {
    attributes
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn field_u32(attributes: &Value, key: &str, default: u32) -> u32 {
    attributes
        .get(key)
        .and_then(Value::as_u64)
        .map(|value| value as u32)
        .unwrap_or(default)
}

fn field_bool(attributes: &Value, key: &str) -> bool {
    attributes
        .get(key)
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// The comment that marks a record as ours.
const MARKER_COMMENT: &str = "nod:managed";

fn create_record(client: &Cloudflare, zone_id: &str, attributes: &Value) -> Result<(), CfError> {
    client
        .create_record(
            zone_id,
            &field_str(attributes, "type"),
            &field_str(attributes, "name"),
            &field_str(attributes, "content"),
            field_u32(attributes, "ttl", 1),
            field_bool(attributes, "proxied"),
            MARKER_COMMENT,
        )
        .map(|_| ())
}

fn update_record(client: &Cloudflare, zone_id: &str, attributes: &Value) -> Result<(), CfError> {
    client.update_record(
        zone_id,
        &field_str(attributes, "record_id"),
        &field_str(attributes, "type"),
        &field_str(attributes, "name"),
        &field_str(attributes, "content"),
        field_u32(attributes, "ttl", 1),
        field_bool(attributes, "proxied"),
        MARKER_COMMENT,
    )
}

fn delete_record(client: &Cloudflare, zone_id: &str, attributes: &Value) -> Result<(), CfError> {
    client.delete_record(zone_id, &field_str(attributes, "record_id"))
}

/// Stamps the ownership marker on existing records, changing nothing else.
fn adopt(request: &Value) -> Value {
    let settings = match settings(request) {
        Ok(settings) => settings,
        Err(message) => return error_message("config", &message),
    };
    let client = Cloudflare::new(settings.token);
    let zone_id = match client.zone_id(&settings.zone) {
        Ok(zone_id) => zone_id,
        Err(error) => return api_error(&error),
    };

    let empty = Vec::new();
    let resources = request
        .get("resources")
        .and_then(Value::as_array)
        .unwrap_or(&empty);

    let mut adopted = Vec::new();
    for resource in resources {
        let id = resource
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let attributes = resource
            .get("attributes")
            .cloned()
            .unwrap_or_else(|| json!({}));

        adopted.push(match stamp_record(&client, &zone_id, &attributes) {
            Ok(()) => json!({ "id": id, "ok": true }),
            Err(error) => json!({ "id": id, "ok": false, "message": error.detail() }),
        });
    }

    json!({ "adopted": adopted, "conditions": [] })
}

/// Sets the marker without touching type, name, content, TTL or proxy state.
fn stamp_record(client: &Cloudflare, zone_id: &str, attributes: &Value) -> Result<(), CfError> {
    let record_id = field_str(attributes, "record_id");
    if record_id.is_empty() {
        return Err(CfError::Config(
            "record_id is required to adopt a record".to_string(),
        ));
    }
    client.patch_comment(zone_id, &record_id, MARKER_COMMENT)
}
