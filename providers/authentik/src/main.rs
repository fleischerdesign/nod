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

/// What the token must be allowed to do, in Authentik's own vocabulary.
///
/// Deletion of a group is not listed: `apply` only deletes when the core's
/// policy allows it, and then the missing `authentik_core.delete_group` is
/// reported with the change that needed it. Applications are fully managed —
/// created and deleted by the fleet — so their deletion permission is part of
/// the required set.
const REQUIRED_PERMISSIONS: [&str; 11] = [
    "authentik_core.view_group",
    "authentik_core.view_user",
    "authentik_core.view_application",
    "authentik_core.view_provider",
    "authentik_core.change_group",
    "authentik_core.add_group",
    "authentik_core.add_user",
    "authentik_core.change_user",
    "authentik_core.add_application",
    "authentik_core.change_application",
    "authentik_core.delete_application",
];

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
        "capabilities": ["authentik.group", "authentik.user", "authentik.application"],
        "operations": ["adopt"],
        "required_permissions": REQUIRED_PERMISSIONS,
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

/// A failure message that names the permission the token is missing.
fn failure_message(error: &ApiError, permission: &str) -> String {
    match error {
        ApiError::Auth => format!("credentials rejected; the token needs {permission}"),
        _ => error.message(),
    }
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
        Err(ApiError::Auth) => {
            return error_message(
                "permission",
                "credentials rejected; the token needs authentik_core.view_group",
            )
        }
        Err(error) => return api_error(&error),
    };

    // Membership resolution and the user seed both read the users endpoint. A
    // token without it can still observe groups, so the gap is a condition.
    let mut conditions = Vec::new();
    let users = match client.users() {
        Ok(users) => users,
        Err(error) => {
            conditions.push(json!({
                "kind": if matches!(error, ApiError::Auth) { "permission" } else { error.kind() },
                "message": failure_message(&error, "authentik_core.view_user"),
            }));
            Vec::new()
        }
    };

    // Applications have no marker field; the core owns them through its record
    // (ADR-037). A token without the permission can still reconcile groups, so
    // that gap is a condition. Anything else — a filtered list above all — is a
    // configuration error: planning against a partial observation would treat
    // invisible applications as absent (ADR-038).
    let applications = match client.applications() {
        Ok(applications) => applications,
        Err(error) if matches!(error, ApiError::Auth) => {
            conditions.push(json!({
                "kind": "permission",
                "message": failure_message(&error, "authentik_core.view_application"),
            }));
            Vec::new()
        }
        Err(error) => return api_error(&error),
    };

    let marker = Marker {
        key: settings.marker_key,
        value: settings.marker_value,
    };

    // Access groups come from the policy bindings. Without them the audience of
    // an application is unknown, so a failure here is fatal once applications
    // exist — an empty list would read as "no group has access".
    let bindings = if applications.is_empty() {
        Vec::new()
    } else {
        match client.bindings() {
            Ok(bindings) => bindings,
            Err(error) => return api_error(&error),
        }
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
        .chain(users.iter().map(|user| {
            let resource = model::user_resource(user, &marker);
            json!({
                "kind": resource.kind,
                "id": resource.id,
                "attributes": resource.attributes,
                "owned": resource.owned,
            })
        }))
        .chain(applications.iter().map(|application| {
            let access = model::access_groups(&bindings, &application.pk, &groups);
            let resource = model::application_resource(application, access);
            json!({
                "kind": resource.kind,
                "id": resource.id,
                "attributes": resource.attributes,
                "owned": resource.owned,
            })
        }))
        .collect();

    json!({ "resources": resources, "conditions": conditions })
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
    let changes = match request.get("changes").cloned() {
        Some(value) => match serde_json::from_value::<Vec<model::Change>>(value) {
            Ok(changes) => changes,
            Err(error) => {
                eprintln!("invalid changes: {error}");
                return error_message("protocol", "invalid changes");
            }
        },
        None => Vec::new(),
    };

    if changes.is_empty() {
        return json!({ "applied": [], "conditions": [] });
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
        Err(ApiError::Auth) => {
            return error_message(
                "permission",
                "credentials rejected; the token needs authentik_core.view_group",
            )
        }
        Err(error) => return api_error(&error),
    };
    let mut users = match client.users() {
        Ok(users) => users,
        Err(ApiError::Auth) => {
            return error_message(
                "permission",
                "credentials rejected; the token needs authentik_core.view_user",
            )
        }
        Err(error) => return api_error(&error),
    };

    let marker = Marker {
        key: settings.marker_key,
        value: settings.marker_value,
    };
    let mut applied = Vec::new();
    let mut providers: Option<Vec<api::ProviderSummary>> = None;
    let mut bindings: Option<Vec<api::Binding>> = None;
    let mut applications: Option<Vec<api::Application>> = None;

    for change in &changes {
        let id = change.resource.id.clone();

        if !change.owned {
            applied.push(failed(
                &id,
                "refusing to change an unowned resource",
                "config",
            ));
            continue;
        }

        match (change.kind.as_str(), change.resource.kind.as_str()) {
            ("create", model::KIND_USER) => {
                let name = attribute_str(&change.resource.attributes, "name");
                let email = attribute_str(&change.resource.attributes, "email");
                let attributes = marker.stamp(&json!({}));
                match client.create_user(&id, name, email, &attributes) {
                    Ok(user) => {
                        // A group in the same plan may name this user; resolve it
                        // against what now exists, not the opening snapshot.
                        users.push(user);
                        applied.push(ok(&id));
                    }
                    Err(error) => applied.push(failed(
                        &id,
                        &failure_message(&error, "authentik_core.add_user"),
                        error.kind(),
                    )),
                }
            }
            ("update", model::KIND_USER) => applied.push(failed(
                &id,
                "a user is a seed and is never updated",
                "config",
            )),
            ("delete", model::KIND_USER) => applied.push(failed(
                &id,
                "a user is never deleted by the provider",
                "config",
            )),
            ("create", model::KIND_GROUP) => {
                let user_pks = match resolve_users(&change.resource.attributes, &users) {
                    Ok(pks) => pks,
                    Err(()) => {
                        applied.push(failed(&id, "unknown user in the declaration", "config"));
                        continue;
                    }
                };
                let attributes =
                    marker.stamp(&Value::Object(free_form(&change.resource.attributes)));
                match client.create_group(
                    &id,
                    attribute_bool(&change.resource.attributes, "is_superuser"),
                    &attributes,
                    user_pks.as_deref(),
                ) {
                    Ok(()) => applied.push(ok(&id)),
                    Err(error) => applied.push(failed(
                        &id,
                        &failure_message(&error, "authentik_core.add_group"),
                        error.kind(),
                    )),
                }
            }
            ("update", model::KIND_GROUP) => {
                let Some(group) = groups.iter().find(|group| group.name == id) else {
                    applied.push(failed(&id, "group not found", "config"));
                    continue;
                };
                let user_pks = match resolve_users(&change.resource.attributes, &users) {
                    Ok(pks) => pks,
                    Err(()) => {
                        applied.push(failed(&id, "unknown user in the declaration", "config"));
                        continue;
                    }
                };
                let attributes = merged_attributes(&group.attributes, &change.resource.attributes);
                match client.update_group(
                    &group.pk,
                    attribute_bool(&change.resource.attributes, "is_superuser"),
                    &attributes,
                    user_pks.as_deref(),
                ) {
                    Ok(()) => applied.push(ok(&id)),
                    Err(error) => applied.push(failed(
                        &id,
                        &failure_message(&error, "authentik_core.change_group"),
                        error.kind(),
                    )),
                }
            }
            ("delete", model::KIND_GROUP) => {
                let Some(group) = groups.iter().find(|group| group.name == id) else {
                    applied.push(json!({ "id": id, "ok": true, "message": "already absent" }));
                    continue;
                };
                match client.delete_group(&group.pk) {
                    Ok(()) => applied.push(ok(&id)),
                    Err(error) => applied.push(failed(
                        &id,
                        &failure_message(&error, "authentik_core.delete_group"),
                        error.kind(),
                    )),
                }
            }
            (kind @ ("create" | "update"), model::KIND_APPLICATION) => {
                if kind == "create" && attribute_str(&change.resource.attributes, "name").is_empty()
                {
                    applied.push(failed(&id, "an application needs a `name`", "config"));
                    continue;
                }

                // Provider names resolve to primary keys; the declaration never
                // carries a uuid, and an unknown name is a declaration error.
                if providers.is_none() {
                    match client.providers() {
                        Ok(list) => providers = Some(list),
                        Err(error) => {
                            applied.push(failed(
                                &id,
                                &failure_message(&error, "authentik_core.view_provider"),
                                error.kind(),
                            ));
                            continue;
                        }
                    }
                }
                let known = providers.as_deref().unwrap_or(&[]);
                let body = match application_body(&change.resource.attributes, known) {
                    Ok(body) => body,
                    Err(message) => {
                        applied.push(failed(&id, &message, "config"));
                        continue;
                    }
                };

                let access = match declared_access_groups(&change.resource.attributes) {
                    Ok(access) => access,
                    Err(message) => {
                        applied.push(failed(&id, &message, "config"));
                        continue;
                    }
                };

                if kind == "create" {
                    match client.create_application(&body) {
                        Ok(created) => {
                            if let Some(declared) = &access {
                                if let Err(message) =
                                    reconcile_bindings(&client, &created.pk, declared, &groups, &[])
                                {
                                    // An application without its audience is open to
                                    // everyone (`core_default_app_access`). Remove it
                                    // rather than leave an unowned, open object behind.
                                    let removed = client.delete_application(&id).is_ok();
                                    let note = if removed {
                                        message
                                    } else {
                                        format!(
                                            "{message}; the compensating delete also failed, the application stays unowned and open"
                                        )
                                    };
                                    applied.push(failed(&id, &note, "config"));
                                    continue;
                                }
                            }
                            applied.push(ok(&id));
                        }
                        Err(error) => applied.push(failed(
                            &id,
                            &failure_message(&error, "authentik_core.add_application"),
                            error.kind(),
                        )),
                    }
                    continue;
                }

                match client.update_application(&id, &body) {
                    Ok(()) => {
                        if let Some(declared) = &access {
                            // The binding target is the application's pk, which the
                            // declaration never carries; look it up once.
                            if applications.is_none() {
                                match client.applications() {
                                    Ok(list) => applications = Some(list),
                                    Err(error) => {
                                        applied.push(failed(
                                            &id,
                                            &failure_message(
                                                &error,
                                                "authentik_core.view_application",
                                            ),
                                            error.kind(),
                                        ));
                                        continue;
                                    }
                                }
                            }
                            let target = applications
                                .as_deref()
                                .unwrap_or(&[])
                                .iter()
                                .find(|application| application.slug == id)
                                .map(|application| application.pk.clone());
                            let Some(target) = target else {
                                applied.push(failed(
                                    &id,
                                    "application not found after the update",
                                    "config",
                                ));
                                continue;
                            };
                            if bindings.is_none() {
                                match client.bindings() {
                                    Ok(list) => bindings = Some(list),
                                    Err(error) => {
                                        applied.push(failed(
                                            &id,
                                            &failure_message(
                                                &error,
                                                "authentik_policies.view_policybinding",
                                            ),
                                            error.kind(),
                                        ));
                                        continue;
                                    }
                                }
                            }
                            let known = bindings.as_deref().unwrap_or(&[]);
                            if let Err(message) =
                                reconcile_bindings(&client, &target, declared, &groups, known)
                            {
                                applied.push(failed(&id, &message, "config"));
                                continue;
                            }
                        }
                        applied.push(ok(&id));
                    }
                    Err(error) => applied.push(failed(
                        &id,
                        &failure_message(&error, "authentik_core.change_application"),
                        error.kind(),
                    )),
                }
            }
            ("delete", model::KIND_APPLICATION) => match client.delete_application(&id) {
                Ok(()) => applied.push(ok(&id)),
                Err(error) => applied.push(failed(
                    &id,
                    &failure_message(&error, "authentik_core.delete_application"),
                    error.kind(),
                )),
            },
            (other, _) => applied.push(failed(
                &id,
                &format!("unsupported change kind: {other}"),
                "protocol",
            )),
        }
    }

    json!({ "applied": applied, "conditions": [] })
}

fn ok(id: &str) -> Value {
    json!({ "id": id, "ok": true, "message": "" })
}

fn failed(id: &str, message: &str, kind: &str) -> Value {
    json!({
        "id": id,
        "ok": false,
        "message": message,
        "error": { "kind": kind, "message": message }
    })
}

fn attribute_bool(attributes: &Value, key: &str) -> Option<bool> {
    attributes.get(key).and_then(Value::as_bool)
}

fn attribute_str<'a>(attributes: &'a Value, key: &str) -> &'a str {
    attributes.get(key).and_then(Value::as_str).unwrap_or("")
}

/// The keys the nod resource carries for the provider's own control, never for
/// the object's free-form `attributes` object.
const CONTROL_KEYS: [&str; 2] = ["is_superuser", "users"];

/// The declared free-form attributes, without the control keys.
///
/// Authentik stores the description inside `attributes`, while `is_superuser`
/// and `users` are top-level group fields; mixing them would put two API
/// fields into the attributes object.
fn free_form(declared: &Value) -> serde_json::Map<String, Value> {
    declared
        .as_object()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|(key, _)| !CONTROL_KEYS.contains(&key.as_str()))
        .collect()
}

/// The attributes an update writes: the observed object with every declared
/// free-form key overwritten, so an undeclared attribute is preserved.
fn merged_attributes(observed: &Value, declared: &Value) -> Value {
    let mut object = observed.as_object().cloned().unwrap_or_default();
    for (key, value) in free_form(declared) {
        object.insert(key, value);
    }
    Value::Object(object)
}

/// Builds an application's create or patch body from the declared attributes.
///
/// Only declared keys are sent: a partial declaration must not reset a field it
/// does not name. `provider` names a provider the way a human knows it and is
/// resolved to the pk the API expects; an empty name detaches it.
fn application_body(
    attributes: &Value,
    providers: &[api::ProviderSummary],
) -> Result<Value, String> {
    let mut body = serde_json::Map::new();

    for key in [
        "name",
        "meta_launch_url",
        "meta_description",
        "meta_publisher",
        "group",
        "policy_engine_mode",
    ] {
        if let Some(value) = attributes.get(key) {
            if !value.is_string() {
                return Err(format!("`{key}` must be a string"));
            }
            body.insert(key.to_string(), value.clone());
        }
    }

    for key in ["open_in_new_tab", "meta_hide"] {
        if let Some(value) = attributes.get(key) {
            let Some(flag) = value.as_bool() else {
                return Err(format!("`{key}` must be a boolean"));
            };
            body.insert(key.to_string(), json!(flag));
        }
    }

    if let Some(value) = attributes.get("provider") {
        if value.is_null() || value.as_str() == Some("") {
            body.insert("provider".to_string(), Value::Null);
        } else if let Some(name) = value.as_str() {
            let Some(provider) = providers.iter().find(|provider| provider.name == name) else {
                return Err(format!("unknown provider `{name}`"));
            };
            body.insert("provider".to_string(), json!(provider.pk));
        } else {
            return Err("`provider` must be a string or null".to_string());
        }
    }

    Ok(Value::Object(body))
}

/// The declared access groups, or `None` when the declaration does not manage
/// them. A wrong type is a configuration error, never an empty set.
fn declared_access_groups(attributes: &Value) -> Result<Option<Vec<String>>, String> {
    match attributes.get("access_groups") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| "`access_groups` must be a list of group names".to_string())
            })
            .collect::<Result<Vec<String>, String>>()
            .map(Some),
        Some(_) => Err("`access_groups` must be a list of group names".to_string()),
    }
}

/// Reconciles the group bindings of one application against the declaration.
///
/// The declared list is the complete set of group bindings; user and policy
/// bindings belong to someone else and are never touched. Grants are made
/// before removals, so a failure never leaves the application less accessible
/// than either state, and a duplicate or disabled binding for a declared group
/// is removed once its enabled replacement exists.
fn reconcile_bindings(
    client: &Authentik,
    target: &str,
    declared: &[String],
    groups: &[model::Group],
    bindings: &[api::Binding],
) -> Result<(), String> {
    let mut wanted: Vec<&str> = Vec::new();
    for name in declared {
        let Some(group) = groups.iter().find(|group| &group.name == name) else {
            return Err(format!("unknown group `{name}` in `access_groups`"));
        };
        wanted.push(group.pk.as_str());
    }

    let existing: Vec<&api::Binding> = bindings
        .iter()
        .filter(|binding| binding.target.as_deref() == Some(target))
        .filter(|binding| {
            binding.group.is_some() && binding.user.is_none() && binding.policy.is_none()
        })
        .collect();

    let mut created: Vec<String> = Vec::new();
    for pk in &wanted {
        let satisfied = existing
            .iter()
            .any(|binding| binding.enabled && binding.group.as_deref() == Some(pk));
        if satisfied {
            continue;
        }
        match client.create_binding(target, pk) {
            Ok(binding) => created.push(binding.pk),
            Err(error) => {
                for binding in &created {
                    let _ = client.delete_binding(binding);
                }
                return Err(format!(
                    "cannot grant group access: {}",
                    failure_message(&error, "authentik_policies.add_policybinding")
                ));
            }
        }
    }

    let mut kept: Vec<&str> = Vec::new();
    for binding in &existing {
        let Some(pk) = binding.group.as_deref() else {
            continue;
        };
        let declared_group = wanted.contains(&pk);
        let duplicate = kept.contains(&pk);
        if declared_group && binding.enabled && !duplicate {
            kept.push(pk);
            continue;
        }
        if let Err(error) = client.delete_binding(&binding.pk) {
            return Err(format!(
                "cannot remove group access: {}",
                failure_message(&error, "authentik_policies.delete_policybinding")
            ));
        }
    }

    Ok(())
}

/// Resolves the declared member usernames to primary keys.
///
/// `Ok(None)` means the declaration does not name membership (`members = null`),
/// so the reconcile must not write it. `Err` means a username does not exist:
/// sending the rest would silently drop a member the declaration asks for.
fn resolve_users(attributes: &Value, users: &[api::User]) -> Result<Option<Vec<i64>>, ()> {
    let Some(declared) = attributes.get("users").and_then(Value::as_array) else {
        return Ok(None);
    };
    declared
        .iter()
        .filter_map(Value::as_str)
        .map(|username| {
            users
                .iter()
                .find(|user| user.username == username)
                .map(|user| user.pk)
                .ok_or(())
        })
        .collect::<Result<Vec<i64>, ()>>()
        .map(Some)
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
    let users = match client.users() {
        Ok(users) => users,
        Err(error) => return api_error(&error),
    };
    // Adoption of an application writes nothing; the object must exist, and the
    // core records the operator's decision. The list also proves the account
    // sees every application (ADR-038).
    let applications = match client.applications() {
        Ok(applications) => applications,
        Err(error) => return api_error(&error),
    };

    let marker = Marker {
        key: settings.marker_key,
        value: settings.marker_value,
    };
    let mut adopted = Vec::new();

    for resource in &resources {
        match resource.kind.as_str() {
            model::KIND_GROUP => {
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
            model::KIND_USER => {
                let Some(user) = users.iter().find(|user| user.username == resource.id) else {
                    adopted.push(json!({
                        "id": resource.id,
                        "ok": false,
                        "message": "user not found",
                        "error": { "kind": "config", "message": "user not found" }
                    }));
                    continue;
                };

                // Adoption stamps the marker and nothing else: the profile and
                // every other attribute stay exactly as the API returned them.
                let attributes = marker.stamp(&user.attributes);
                match client.patch_user(user.pk, &attributes) {
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
            model::KIND_APPLICATION => {
                if applications
                    .iter()
                    .any(|application| application.slug == resource.id)
                {
                    adopted.push(json!({
                        "id": resource.id,
                        "ok": true,
                        "message": "no marker field; the core records ownership"
                    }));
                } else {
                    adopted.push(json!({
                        "id": resource.id,
                        "ok": false,
                        "message": "application not found",
                        "error": { "kind": "config", "message": "application not found" }
                    }));
                }
            }
            other => adopted.push(json!({
                "id": resource.id,
                "ok": false,
                "message": format!("unsupported kind: {other}"),
                "error": { "kind": "protocol", "message": format!("unsupported kind: {other}") }
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
        assert!(handshake["capabilities"]
            .as_array()
            .unwrap()
            .contains(&json!("authentik.application")));
        assert_eq!(handshake["ownership"], json!("marker"));
        assert_eq!(handshake["operations"][0], json!("adopt"));
    }

    fn provider(name: &str, pk: i64) -> api::ProviderSummary {
        api::ProviderSummary {
            pk,
            name: name.to_string(),
        }
    }

    #[test]
    fn an_application_body_resolves_a_provider_name_to_its_pk() {
        let body = application_body(
            &json!({ "name": "Grafana", "provider": "grafana-oauth" }),
            &[provider("grafana-oauth", 7)],
        )
        .expect("a known provider resolves");
        assert_eq!(body["provider"], json!(7));
        assert_eq!(body["name"], json!("Grafana"));
    }

    #[test]
    fn an_application_body_rejects_an_unknown_provider() {
        let error = application_body(&json!({ "provider": "missing" }), &[]).unwrap_err();
        assert!(error.contains("unknown provider"), "{error}");
    }

    #[test]
    fn an_application_body_detaches_on_an_empty_provider() {
        let body = application_body(&json!({ "provider": "" }), &[]).expect("detach");
        assert_eq!(body["provider"], Value::Null);
    }

    #[test]
    fn an_application_body_sends_only_declared_keys() {
        let body = application_body(&json!({ "name": "Grafana" }), &[]).expect("body");
        assert_eq!(body.as_object().unwrap().len(), 1);
        assert!(body.get("meta_launch_url").is_none());
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
    fn a_non_empty_apply_needs_settings() {
        let response = apply(&json!({
            "changes": [{
                "kind": "create",
                "owned": true,
                "destructive": false,
                "resource": { "kind": "authentik.group", "id": "new", "attributes": {} }
            }]
        }));
        assert_eq!(response["error"]["kind"], json!("config"));
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
