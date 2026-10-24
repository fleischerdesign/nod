//! Pure mapping and planning for the Authentik groups provider.
//!
//! The protocol and the HTTP client are I/O; this module is neither. It maps
//! Authentik groups to nod resources and computes the plan, so the rules can be
//! tested without a token, an instance or a network.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Resource kind for an Authentik group.
pub const KIND_GROUP: &str = "authentik.group";

/// Resource kind for an Authentik user.
pub const KIND_USER: &str = "authentik.user";

/// Resource kind for an Authentik application.
///
/// An application has no free-form field, so it never carries a marker: the
/// core owns it through its record (ADR-037) and the provider always reports
/// `owned: false` here.
pub const KIND_APPLICATION: &str = "authentik.application";

/// A user as the group serializer's `users_obj` returns it.
#[derive(Debug, Clone, Deserialize)]
pub struct PartialUser {
    /// Login name; the provider's membership identifier.
    pub username: String,
}

/// A group as Authentik returns it.
#[derive(Debug, Clone, Deserialize)]
pub struct Group {
    /// Authentik's UUID primary key; used by `adopt` and `apply` to address the object.
    pub pk: String,
    /// Unique group name; the provider's stable id.
    pub name: String,
    /// Whether members become superusers.
    #[serde(default)]
    pub is_superuser: bool,
    /// Free-form attributes; the ownership marker and the description live here.
    #[serde(default)]
    pub attributes: Value,
    /// Current members as partial users, including usernames.
    #[serde(default)]
    pub users_obj: Vec<PartialUser>,
}

/// The ownership marker, configured per provider instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Marker {
    /// Attribute key, e.g. `nod`.
    pub key: String,
    /// Attribute value, e.g. `owned`.
    pub value: String,
}

impl Marker {
    /// Whether `attributes` carries this marker.
    pub fn matches_attributes(&self, attributes: &Value) -> bool {
        attributes.get(&self.key).and_then(Value::as_str) == Some(self.value.as_str())
    }

    /// Whether `group` carries this marker.
    pub fn matches(&self, group: &Group) -> bool {
        self.matches_attributes(&group.attributes)
    }

    /// Returns a copy of `attributes` with the marker set, preserving the rest.
    ///
    /// Adoption writes the marker and nothing else: a group's description,
    /// its free-form attributes and its superuser flag stay as they are.
    pub fn stamp(&self, attributes: &Value) -> Value {
        let mut object = attributes.as_object().cloned().unwrap_or_default();
        object.insert(self.key.clone(), Value::String(self.value.clone()));
        Value::Object(object)
    }
}

/// A nod resource, as the protocol defines it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Resource {
    /// Resource kind, e.g. `authentik.group`.
    pub kind: String,
    /// Provider-scoped identifier; the group name.
    pub id: String,
    /// Provider-specific attributes.
    #[serde(default)]
    pub attributes: Value,
}

/// An observed resource.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ObservedResource {
    /// Resource kind.
    pub kind: String,
    /// Provider-scoped identifier.
    pub id: String,
    /// Provider-specific attributes.
    #[serde(default)]
    pub attributes: Value,
    /// Whether the fleet owns it.
    #[serde(default)]
    pub owned: bool,
}

/// One proposed change.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Change {
    /// `create`, `update` or `delete`.
    pub kind: String,
    /// Whether the fleet owns the target.
    pub owned: bool,
    /// Whether the change removes something.
    pub destructive: bool,
    /// The resource the change addresses.
    pub resource: Resource,
}

/// Maps a group to the resource the core sees.
///
/// The name is the id; `is_superuser`, the description and the member
/// usernames are the attributes. The ownership marker and any other free-form
/// attribute stay out of the comparison, so a marker write never looks like a
/// desired-state change.
pub fn to_resource(group: &Group, marker: &Marker) -> ObservedResource {
    let description = group
        .attributes
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("");
    let mut users: Vec<&str> = group
        .users_obj
        .iter()
        .map(|user| user.username.as_str())
        .collect();
    users.sort_unstable();

    ObservedResource {
        kind: KIND_GROUP.to_string(),
        id: group.name.clone(),
        attributes: json!({
            "is_superuser": group.is_superuser,
            "description": description,
            "users": users,
        }),
        owned: marker.matches(group),
    }
}

/// Maps an application to the resource the core sees.
///
/// The slug is the id; the display name, the provider's *name* (never its pk),
/// the launch URL and the tile metadata are the attributes. A declaration can
/// therefore name a provider the way a human knows it.
pub fn application_resource(app: &crate::api::Application) -> ObservedResource {
    ObservedResource {
        kind: KIND_APPLICATION.to_string(),
        id: app.slug.clone(),
        attributes: json!({
            "name": app.name,
            "provider": app
                .provider_obj
                .as_ref()
                .map(|provider| provider.name.clone())
                .unwrap_or_default(),
            "meta_launch_url": app.meta_launch_url,
            "meta_description": app.meta_description,
            "meta_publisher": app.meta_publisher,
            "open_in_new_tab": app.open_in_new_tab,
            "policy_engine_mode": app.policy_engine_mode,
            "group": app.group,
            "meta_hide": app.meta_hide,
        }),
        owned: false,
    }
}

/// Whether the declaration asks for anything the observation does not show.
///
/// Only the keys the declaration names are compared. `members = null` in the
/// directory contract means membership stays UI-owned, so the declaration omits
/// `users`; a reconcile must then neither compare nor write it. An explicitly
/// empty `users: []` is a declaration and does remove every member.
fn differs(declared: &Value, observed: &Value) -> bool {
    match declared.as_object() {
        Some(want) => want
            .iter()
            .any(|(key, value)| observed.get(key) != Some(value)),
        None => declared != observed,
    }
}

/// Maps a user to the resource the core sees.
///
/// The username is the id; the display name and the e-mail are the attributes.
/// The marker lives in the user's free-form attributes. A user is a seed: the
/// plan never updates or deletes one, because credentials, profile edits and
/// the account itself belong to the person.
pub fn user_resource(user: &crate::api::User, marker: &Marker) -> ObservedResource {
    ObservedResource {
        kind: KIND_USER.to_string(),
        id: user.username.clone(),
        attributes: json!({
            "name": user.name,
            "email": user.email,
        }),
        owned: marker.matches_attributes(&user.attributes),
    }
}

/// Computes the plan from a declaration and an observation.
///
/// A foreign resource is never updated or deleted: adoption is an explicit
/// operation, not something a reconcile invents (ADR-032).
pub fn plan(desired: &[Resource], observed: &[ObservedResource]) -> Vec<Change> {
    let mut changes = Vec::new();

    // Users first: a group's membership can only resolve against a user that
    // exists. A user is a seed - created when missing, never updated, never
    // deleted.
    for want in desired.iter().filter(|resource| resource.kind == KIND_USER) {
        let exists = observed
            .iter()
            .any(|current| current.kind == want.kind && current.id == want.id);
        if !exists {
            changes.push(Change {
                kind: "create".to_string(),
                owned: true,
                destructive: false,
                resource: want.clone(),
            });
        }
    }

    // Groups and applications are managed the same way: created when missing,
    // updated when a declared attribute differs, left alone when foreign. A
    // group carries its marker; an application is owned through the core's
    // record, so the provider only reports what it sees (ADR-037).
    const MANAGED: [&str; 2] = [KIND_GROUP, KIND_APPLICATION];
    for want in desired
        .iter()
        .filter(|resource| MANAGED.contains(&resource.kind.as_str()))
    {
        match observed
            .iter()
            .find(|current| current.kind == want.kind && current.id == want.id)
        {
            None => changes.push(Change {
                kind: "create".to_string(),
                owned: true,
                destructive: false,
                resource: want.clone(),
            }),
            Some(current) if !current.owned => {}
            Some(current) if differs(&want.attributes, &current.attributes) => {
                changes.push(Change {
                    kind: "update".to_string(),
                    owned: true,
                    destructive: false,
                    resource: want.clone(),
                })
            }
            Some(_) => {}
        }
    }

    for current in observed
        .iter()
        .filter(|resource| MANAGED.contains(&resource.kind.as_str()) && resource.owned)
    {
        let declared = desired
            .iter()
            .any(|want| want.kind == current.kind && want.id == current.id);
        if !declared {
            changes.push(Change {
                kind: "delete".to_string(),
                owned: true,
                destructive: true,
                resource: Resource {
                    kind: current.kind.clone(),
                    id: current.id.clone(),
                    attributes: current.attributes.clone(),
                },
            });
        }
    }

    changes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marker() -> Marker {
        Marker {
            key: "nod".to_string(),
            value: "owned".to_string(),
        }
    }

    fn group(name: &str, superuser: bool, attributes: Value) -> Group {
        Group {
            pk: format!("pk-{name}"),
            name: name.to_string(),
            is_superuser: superuser,
            attributes,
            users_obj: Vec::new(),
        }
    }

    fn resource(name: &str, superuser: bool) -> Resource {
        Resource {
            kind: KIND_GROUP.to_string(),
            id: name.to_string(),
            attributes: json!({
                "is_superuser": superuser,
                "description": "",
                "users": [],
            }),
        }
    }

    fn desired_user(username: &str) -> Resource {
        Resource {
            kind: KIND_USER.to_string(),
            id: username.to_string(),
            attributes: json!({ "name": "Display", "email": "display@example.test" }),
        }
    }

    fn api_user(username: &str, name: &str, email: &str, attributes: Value) -> crate::api::User {
        crate::api::User {
            pk: 7,
            username: username.to_string(),
            name: name.to_string(),
            email: email.to_string(),
            attributes,
        }
    }

    #[test]
    fn a_user_maps_username_to_id_and_profile_to_attributes() {
        let mapped = user_resource(
            &api_user("ada", "Ada", "ada@example.test", json!({ "nod": "owned" })),
            &marker(),
        );
        assert_eq!(mapped.kind, KIND_USER);
        assert_eq!(mapped.id, "ada");
        assert_eq!(
            mapped.attributes,
            json!({ "name": "Ada", "email": "ada@example.test" })
        );
        assert!(mapped.owned);
    }

    #[test]
    fn a_missing_user_is_created() {
        let changes = plan(&[desired_user("ada")], &[]);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].kind, "create");
        assert_eq!(changes[0].resource.kind, KIND_USER);
    }

    #[test]
    fn an_existing_user_is_never_updated() {
        let observed = vec![user_resource(
            &api_user(
                "ada",
                "Old name",
                "old@example.test",
                json!({ "nod": "owned" }),
            ),
            &marker(),
        )];
        assert!(
            plan(&[desired_user("ada")], &observed).is_empty(),
            "a user is a seed, not a managed profile"
        );
    }

    #[test]
    fn a_declared_user_that_was_created_at_runtime_is_left_alone() {
        // The declaration decides which accounts the fleet *creates*, never
        // which accounts may exist: an account made in the interface, by
        // enrollment or by an LDAP sync is foreign and is not re-created.
        let observed = vec![user_resource(
            &api_user("ada", "Ada", "ada@example.test", json!({})),
            &marker(),
        )];
        assert!(plan(&[desired_user("ada")], &observed).is_empty());
    }

    #[test]
    fn users_are_planned_before_groups() {
        let mut group = resource("media", false);
        group.attributes["users"] = json!(["ada"]);

        let changes = plan(&[group, desired_user("ada")], &[]);
        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0].resource.kind, KIND_USER);
        assert_eq!(changes[1].resource.kind, KIND_GROUP);
    }

    #[test]
    fn the_marker_is_read_from_attributes() {
        assert!(marker().matches(&group("admins", false, json!({ "nod": "owned" }))));
        assert!(!marker().matches(&group("users", false, json!({ "other": "yes" }))));
        assert!(!marker().matches(&group("empty", false, json!({}))));
    }

    #[test]
    fn a_group_maps_name_to_id_and_superuser_to_attributes() {
        let mapped = to_resource(&group("admins", true, json!({})), &marker());
        assert_eq!(mapped.kind, KIND_GROUP);
        assert_eq!(mapped.id, "admins");
        assert_eq!(
            mapped.attributes,
            json!({ "is_superuser": true, "description": "", "users": [] })
        );
        assert!(!mapped.owned);
    }

    #[test]
    fn a_group_maps_description_and_members_sorted() {
        let mut group = group("admins", false, json!({ "description": "ops" }));
        group.users_obj = vec![
            PartialUser {
                username: "zoe".to_string(),
            },
            PartialUser {
                username: "ada".to_string(),
            },
        ];

        let mapped = to_resource(&group, &marker());
        assert_eq!(mapped.attributes["description"], json!("ops"));
        assert_eq!(mapped.attributes["users"], json!(["ada", "zoe"]));
    }

    #[test]
    fn a_missing_group_is_created() {
        let changes = plan(&[resource("admins", false)], &[]);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].kind, "create");
        assert!(changes[0].owned);
        assert!(!changes[0].destructive);
    }

    #[test]
    fn an_owned_group_with_different_attributes_is_updated() {
        let observed = vec![to_resource(
            &group("admins", false, json!({ "nod": "owned" })),
            &marker(),
        )];
        let changes = plan(&[resource("admins", true)], &observed);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].kind, "update");
    }

    #[test]
    fn an_owned_group_with_equal_attributes_is_unchanged() {
        let observed = vec![to_resource(
            &group("admins", true, json!({ "nod": "owned" })),
            &marker(),
        )];
        assert!(plan(&[resource("admins", true)], &observed).is_empty());
    }

    #[test]
    fn undeclared_membership_is_neither_compared_nor_written() {
        let mut current = group("media", false, json!({ "nod": "owned" }));
        current.users_obj = vec![PartialUser {
            username: "philipp".to_string(),
        }];
        let observed = vec![to_resource(&current, &marker())];

        // `members = null`: the declaration names no `users`, so a UI-owned
        // membership must not read as drift.
        let desired = vec![Resource {
            kind: KIND_GROUP.to_string(),
            id: "media".to_string(),
            attributes: json!({ "is_superuser": false, "description": "" }),
        }];
        assert!(plan(&desired, &observed).is_empty());
    }

    #[test]
    fn a_declared_empty_membership_removes_the_members() {
        let mut current = group("media", false, json!({ "nod": "owned" }));
        current.users_obj = vec![PartialUser {
            username: "philipp".to_string(),
        }];
        let observed = vec![to_resource(&current, &marker())];

        let desired = vec![Resource {
            kind: KIND_GROUP.to_string(),
            id: "media".to_string(),
            attributes: json!({ "is_superuser": false, "description": "", "users": [] }),
        }];
        let changes = plan(&desired, &observed);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].kind, "update");
    }

    #[test]
    fn a_foreign_group_is_never_updated() {
        let observed = vec![to_resource(&group("admins", false, json!({})), &marker())];
        assert!(plan(&[resource("admins", true)], &observed).is_empty());
    }

    #[test]
    fn an_owned_group_outside_the_declaration_is_deleted() {
        let observed = vec![to_resource(
            &group("old", false, json!({ "nod": "owned" })),
            &marker(),
        )];
        let changes = plan(&[], &observed);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].kind, "delete");
        assert!(changes[0].destructive);
    }

    #[test]
    fn a_foreign_group_outside_the_declaration_is_left_alone() {
        let observed = vec![to_resource(&group("foreign", false, json!({})), &marker())];
        assert!(plan(&[], &observed).is_empty());
    }

    #[test]
    fn stamping_preserves_existing_attributes_and_sets_the_marker() {
        let stamped = marker().stamp(&json!({ "description": "media" }));
        assert_eq!(stamped["description"], json!("media"));
        assert_eq!(stamped["nod"], json!("owned"));
    }

    #[test]
    fn stamping_null_attributes_starts_an_object() {
        assert_eq!(marker().stamp(&Value::Null), json!({ "nod": "owned" }));
    }

    fn application(slug: &str, provider: Option<&str>) -> crate::api::Application {
        crate::api::Application {
            slug: slug.to_string(),
            name: format!("App {slug}"),
            provider_obj: provider.map(|name| crate::api::ProviderSummary {
                pk: 7,
                name: name.to_string(),
            }),
            open_in_new_tab: false,
            meta_launch_url: "https://app.example.test".to_string(),
            meta_description: String::new(),
            meta_publisher: String::new(),
            policy_engine_mode: "any".to_string(),
            group: String::new(),
            meta_hide: false,
        }
    }

    fn desired_application(slug: &str, name: &str) -> Resource {
        Resource {
            kind: KIND_APPLICATION.to_string(),
            id: slug.to_string(),
            attributes: json!({ "name": name }),
        }
    }

    #[test]
    fn an_application_maps_slug_to_id_and_provider_name() {
        let mapped = application_resource(&application("grafana", Some("grafana-oauth")));
        assert_eq!(mapped.id, "grafana");
        assert_eq!(mapped.attributes["provider"], json!("grafana-oauth"));
        assert!(
            !mapped.owned,
            "an application has no marker; the core's record owns it"
        );
    }

    #[test]
    fn an_application_without_a_provider_maps_to_an_empty_name() {
        let mapped = application_resource(&application("bookmark", None));
        assert_eq!(mapped.attributes["provider"], json!(""));
    }

    #[test]
    fn a_missing_application_is_created() {
        let changes = plan(&[desired_application("grafana", "App grafana")], &[]);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].kind, "create");
        assert_eq!(changes[0].resource.kind, KIND_APPLICATION);
        assert!(changes[0].owned);
    }

    #[test]
    fn an_owned_application_with_a_different_name_is_updated() {
        let mut observed = application_resource(&application("grafana", None));
        observed.owned = true;
        let changes = plan(&[desired_application("grafana", "Grafana")], &[observed]);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].kind, "update");
    }

    #[test]
    fn an_unowned_application_is_left_alone() {
        let observed = application_resource(&application("grafana", None));
        let changes = plan(&[desired_application("grafana", "Grafana")], &[observed]);
        assert!(
            changes.is_empty(),
            "a foreign application is never modified (ADR-037)"
        );
    }

    #[test]
    fn an_owned_undeclared_application_is_deleted() {
        let mut observed = application_resource(&application("grafana", None));
        observed.owned = true;
        let changes = plan(&[], &[observed]);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].kind, "delete");
        assert!(changes[0].destructive);
    }

    #[test]
    fn other_kinds_are_ignored() {
        let desired = vec![Resource {
            kind: "authentik.outpost".to_string(),
            id: "outpost".to_string(),
            attributes: json!({}),
        }];
        assert!(plan(&desired, &[]).is_empty());
    }
}
