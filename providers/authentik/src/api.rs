//! Authentik REST client for the group endpoints.
//!
//! Only the transport lives here; mapping and planning stay in `model`.

use std::time::Duration;

use serde::Deserialize;

use crate::model::Group;

/// A classified API failure, mapped to the protocol's error kinds.
#[derive(Debug)]
pub enum ApiError {
    /// Credentials rejected (401/403).
    Auth,
    /// Rate limited (429), with the server's retry hint when present.
    RateLimited(Option<u64>),
    /// Transport failure or 5xx.
    Transient(String),
    /// A 4xx the provider cannot repair; settings or request are wrong.
    Config(String),
}

impl ApiError {
    /// The protocol error kind.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Auth => "auth",
            Self::RateLimited(_) => "rate-limited",
            Self::Transient(_) => "transient",
            Self::Config(_) => "config",
        }
    }

    /// A human-readable detail, without the token.
    pub fn message(&self) -> String {
        match self {
            Self::Auth => "credentials rejected".to_string(),
            Self::RateLimited(_) => "rate limited".to_string(),
            Self::Transient(message) | Self::Config(message) => message.clone(),
        }
    }

    /// The server's retry hint, when it gave one.
    pub fn retry_after(&self) -> Option<u64> {
        match self {
            Self::RateLimited(seconds) => *seconds,
            _ => None,
        }
    }
}

/// An application as the applications endpoint returns it.
///
/// The slug is the id nod addresses; `provider_obj` carries the provider's
/// *name*, so a declaration can name it without knowing a uuid.
#[derive(Debug, Clone, Deserialize)]
pub struct Application {
    /// Policy-binding uuid; the value a `PolicyBinding.target` expects.
    pub pk: String,
    /// Unique slug; the provider's stable id and the API's lookup field.
    pub slug: String,
    /// Display name.
    #[serde(default)]
    pub name: String,
    /// The assigned provider, when one is set.
    #[serde(default)]
    pub provider_obj: Option<ProviderSummary>,
    /// Whether the launch URL opens in a new tab.
    #[serde(default)]
    pub open_in_new_tab: bool,
    /// Launch URL.
    #[serde(default)]
    pub meta_launch_url: String,
    /// Description shown under the tile.
    #[serde(default)]
    pub meta_description: String,
    /// Publisher shown under the tile.
    #[serde(default)]
    pub meta_publisher: String,
    /// `any` or `all`.
    #[serde(default)]
    pub policy_engine_mode: String,
    /// Application group, a UI label.
    #[serde(default)]
    pub group: String,
    /// Whether the tile is hidden from "My applications".
    #[serde(default)]
    pub meta_hide: bool,
}

/// A provider as the list endpoints return it.
#[derive(Debug, Clone, Deserialize)]
pub struct ProviderSummary {
    /// Primary key the application's `provider` field expects.
    pub pk: i64,
    /// Unique provider name.
    pub name: String,
    /// The model the row really is; the only honest type source (`/providers/all/`
    /// uses `select_subclasses`, the typed endpoints use a fixed serializer).
    #[serde(default)]
    pub meta_model_name: String,
}

/// A policy binding as the bindings endpoint returns it.
///
/// Only the fields the provider decides on: a group binding grants access to a
/// group, a user or policy binding is a different decision, and `enabled` says
/// whether the binding is evaluated at all.
#[derive(Debug, Clone, Deserialize)]
pub struct Binding {
    /// Binding primary key, used to delete it.
    pub pk: String,
    /// The bound object's policy-binding uuid.
    #[serde(default)]
    pub target: Option<String>,
    /// Group primary key, when this is a group binding.
    #[serde(default)]
    pub group: Option<String>,
    /// User primary key, when this is a user binding.
    #[serde(default)]
    pub user: Option<String>,
    /// Policy primary key, when this is a policy binding.
    #[serde(default)]
    pub policy: Option<String>,
    /// Whether the binding is evaluated.
    #[serde(default)]
    pub enabled: bool,
}

/// One page of the Authentik binding list response.
#[derive(Debug, Default, Deserialize)]
struct BindingPage {
    #[serde(default)]
    results: Vec<Binding>,
    #[serde(default)]
    pagination: Pagination,
}

/// An outpost as the instances endpoint returns it.
#[derive(Debug, Clone, Deserialize)]
pub struct Outpost {
    /// Outpost uuid.
    pub pk: String,
    /// Unique name; the outpost's stable id.
    pub name: String,
    /// `proxy`, `ldap`, `radius` or `rac`.
    #[serde(rename = "type", default)]
    pub kind: String,
    /// authentik's own marker for a managed outpost; the embedded one is
    /// recreated on startup and must not be deleted.
    #[serde(default)]
    pub managed: Option<String>,
    /// Provider primary keys this outpost serves.
    #[serde(default)]
    pub providers: Vec<i64>,
    /// The outpost's configuration object.
    #[serde(default)]
    pub config: serde_json::Value,
}

/// One page of the outpost list.
#[derive(Debug, Default, Deserialize)]
struct OutpostPage {
    #[serde(default)]
    results: Vec<Outpost>,
    #[serde(default)]
    pagination: Pagination,
}

/// A proxy provider as the proxy endpoint returns it.
#[derive(Debug, Clone, Deserialize)]
pub struct ProxyProvider {
    /// Primary key the API addresses.
    pub pk: i64,
    /// Unique provider name; the provider's stable id.
    pub name: String,
    /// `forward_single` or `forward_domain`.
    #[serde(default)]
    pub mode: String,
    /// The public host the outpost proxies.
    #[serde(default)]
    pub external_host: String,
    /// Cookie domain, empty for a single host.
    #[serde(default)]
    pub cookie_domain: String,
    /// Authorization flow uuid.
    #[serde(default)]
    pub authorization_flow: Option<String>,
    /// Invalidation flow uuid.
    #[serde(default)]
    pub invalidation_flow: Option<String>,
}

/// A flow as the instances endpoint returns it; the declaration names flows by
/// slug, the API expects the uuid.
#[derive(Debug, Clone, Deserialize)]
pub struct Flow {
    /// Flow uuid.
    pub pk: String,
    /// Stable slug.
    pub slug: String,
}

/// One page of the proxy provider list.
#[derive(Debug, Default, Deserialize)]
struct ProxyProviderPage {
    #[serde(default)]
    results: Vec<ProxyProvider>,
    #[serde(default)]
    pagination: Pagination,
}

/// One page of the flow list.
#[derive(Debug, Default, Deserialize)]
struct FlowPage {
    #[serde(default)]
    results: Vec<Flow>,
    #[serde(default)]
    pagination: Pagination,
}

/// One page of the Authentik application list response.
#[derive(Debug, Default, Deserialize)]
struct ApplicationPage {
    #[serde(default)]
    results: Vec<Application>,
    #[serde(default)]
    pagination: Pagination,
}

/// One page of the Authentik provider list response.
#[derive(Debug, Default, Deserialize)]
struct ProviderPage {
    #[serde(default)]
    results: Vec<ProviderSummary>,
    #[serde(default)]
    pagination: Pagination,
}

/// The model name of a true OAuth2 provider, as `/providers/all/` reports it.
const OAUTH2_MODEL: &str = "authentik_providers_oauth2.oauth2provider";

/// An OAuth2 provider as the OAuth2 endpoint returns it.
///
/// `client_secret` is deliberately absent: the API returns it, the provider
/// must never carry it into observation, a plan or a report (ADR-043).
#[derive(Debug, Clone, Deserialize)]
pub struct OAuth2Provider {
    /// Primary key the API addresses.
    pub pk: i64,
    /// Unique provider name; the stable id.
    pub name: String,
    /// Public client id.
    #[serde(default)]
    pub client_id: String,
    /// Authorization flow uuid.
    #[serde(default)]
    pub authorization_flow: Option<String>,
    /// Invalidation flow uuid.
    #[serde(default)]
    pub invalidation_flow: Option<String>,
    /// Registered redirect URIs.
    #[serde(default)]
    pub redirect_uris: Vec<RedirectUri>,
    /// Subject mode, e.g. `hashed_user_id`.
    #[serde(default)]
    pub sub_mode: String,
    /// Whether claims are included in the id token.
    #[serde(default)]
    pub include_claims_in_id_token: bool,
    /// Granted OAuth2 flows.
    #[serde(default)]
    pub grant_types: Vec<String>,
    /// Access-token lifetime, e.g. `hours=1`.
    #[serde(default)]
    pub access_token_validity: String,
    /// Property (scope) mapping uuids.
    #[serde(default)]
    pub property_mappings: Vec<String>,
}

/// One redirect URI as the API returns it.
#[derive(Debug, Clone, Deserialize)]
pub struct RedirectUri {
    /// The URL.
    pub url: String,
}

/// A scope mapping, for resolving managed scope keys.
#[derive(Debug, Clone, Deserialize)]
pub struct ScopeMapping {
    /// Mapping uuid.
    pub pk: String,
    /// Human name.
    #[serde(default)]
    pub name: String,
    /// Managed key, e.g. `goauthentik.io/providers/oauth2/scope-openid`.
    #[serde(default)]
    pub managed: Option<String>,
}

/// One page of the OAuth2 provider list.
#[derive(Debug, Default, Deserialize)]
struct OAuth2ProviderPage {
    #[serde(default)]
    results: Vec<OAuth2Provider>,
    #[serde(default)]
    pagination: Pagination,
}

/// One page of the scope mapping list.
#[derive(Debug, Default, Deserialize)]
struct ScopeMappingPage {
    #[serde(default)]
    results: Vec<ScopeMapping>,
    #[serde(default)]
    pagination: Pagination,
}

/// One page of the Authentik list response.
#[derive(Debug, Default, Deserialize)]
struct Page {
    #[serde(default)]
    results: Vec<Group>,
    #[serde(default)]
    pagination: Pagination,
}

/// A user as the users endpoint returns it.
#[derive(Debug, Clone, Deserialize)]
pub struct User {
    /// User primary key; the value a group's `users` list expects.
    pub pk: i64,
    /// Login name.
    pub username: String,
    /// Display name.
    #[serde(default)]
    pub name: String,
    /// E-mail address.
    #[serde(default)]
    pub email: String,
    /// Free-form attributes; the ownership marker lives here.
    #[serde(default)]
    pub attributes: serde_json::Value,
}

/// One page of the Authentik user list response.
#[derive(Debug, Default, Deserialize)]
struct UserPage {
    #[serde(default)]
    results: Vec<User>,
    #[serde(default)]
    pagination: Pagination,
}

/// Authentik's pagination block.
#[derive(Debug, Default, Deserialize)]
struct Pagination {
    #[serde(default)]
    next: Option<u64>,
    /// Total number of objects the paginator counted, before any per-object
    /// filtering. Used to detect a filtered page (ADR-038).
    #[serde(default)]
    count: Option<u64>,
}

/// The Authentik API client, scoped to the endpoints this provider owns.
pub struct Authentik {
    base: String,
    token: String,
    page_size: u32,
    agent: ureq::Agent,
}

impl Authentik {
    /// Builds a client. The token stays in memory and never enters a message.
    pub fn new(base: &str, token: String, page_size: u32, timeout: Duration) -> Self {
        let agent = ureq::AgentBuilder::new().timeout(timeout).build();
        Self {
            base: base.trim_end_matches('/').to_string(),
            token,
            page_size: page_size.max(1),
            agent,
        }
    }

    /// Reads the complete group list, following pagination to the end.
    pub fn groups(&self) -> Result<Vec<Group>, ApiError> {
        let mut groups = Vec::new();
        let mut page = 1u64;

        loop {
            let url = format!(
                "{}/api/v3/core/groups/?page={}&page_size={}&include_users=true",
                self.base, page, self.page_size
            );
            let body: Page = self.get(&url)?;
            groups.extend(body.results);
            match body.pagination.next {
                Some(next) if next > page => page = next,
                _ => break,
            }
        }

        Ok(groups)
    }

    /// Reads the complete user list, following pagination to the end.
    ///
    /// `apply` needs the pk for every username a group declaration names,
    /// including users who are not yet members of that group.
    pub fn users(&self) -> Result<Vec<User>, ApiError> {
        let mut users = Vec::new();
        let mut page = 1u64;
        loop {
            let url = format!(
                "{}/api/v3/core/users/?page={}&page_size={}",
                self.base, page, self.page_size
            );
            let body: UserPage = self.get(&url)?;
            users.extend(body.results);
            match body.pagination.next {
                Some(next) if next > page => page = next,
                _ => break,
            }
        }
        Ok(users)
    }

    /// Reads the complete outpost list.
    pub fn outposts(&self) -> Result<Vec<Outpost>, ApiError> {
        let mut outposts = Vec::new();
        let mut page = 1u64;
        loop {
            let url = format!(
                "{}/api/v3/outposts/instances/?page={}&page_size={}",
                self.base, page, self.page_size
            );
            let body: OutpostPage = self.get(&url)?;
            outposts.extend(body.results);
            match body.pagination.next {
                Some(next) if next > page => page = next,
                _ => break,
            }
        }
        Ok(outposts)
    }

    /// Creates an outpost; the body holds only declared fields.
    pub fn create_outpost(&self, body: &serde_json::Value) -> Result<(), ApiError> {
        let url = format!("{}/api/v3/outposts/instances/", self.base);
        self.write("POST", &url, body.clone())
    }

    /// Updates the declared fields of an outpost.
    pub fn update_outpost(&self, pk: &str, body: &serde_json::Value) -> Result<(), ApiError> {
        let url = format!("{}/api/v3/outposts/instances/{}/", self.base, pk);
        self.write("PATCH", &url, body.clone())
    }

    /// Deletes an outpost.
    pub fn delete_outpost(&self, pk: &str) -> Result<(), ApiError> {
        let url = format!("{}/api/v3/outposts/instances/{}/", self.base, pk);
        self.write("DELETE", &url, serde_json::Value::Null)
    }

    /// Reads the complete proxy provider list.
    pub fn proxy_providers(&self) -> Result<Vec<ProxyProvider>, ApiError> {
        let mut providers = Vec::new();
        let mut page = 1u64;
        loop {
            let url = format!(
                "{}/api/v3/providers/proxy/?page={}&page_size={}",
                self.base, page, self.page_size
            );
            let body: ProxyProviderPage = self.get(&url)?;
            providers.extend(body.results);
            match body.pagination.next {
                Some(next) if next > page => page = next,
                _ => break,
            }
        }
        Ok(providers)
    }

    /// Reads the complete flow list, for slug-to-uuid resolution.
    pub fn flows(&self) -> Result<Vec<Flow>, ApiError> {
        let mut flows = Vec::new();
        let mut page = 1u64;
        loop {
            let url = format!(
                "{}/api/v3/flows/instances/?page={}&page_size={}",
                self.base, page, self.page_size
            );
            let body: FlowPage = self.get(&url)?;
            flows.extend(body.results);
            match body.pagination.next {
                Some(next) if next > page => page = next,
                _ => break,
            }
        }
        Ok(flows)
    }

    /// Creates a proxy provider; the body holds only declared fields.
    pub fn create_proxy_provider(&self, body: &serde_json::Value) -> Result<(), ApiError> {
        let url = format!("{}/api/v3/providers/proxy/", self.base);
        self.write("POST", &url, body.clone())
    }

    /// Updates the declared fields of a proxy provider.
    pub fn update_proxy_provider(&self, pk: i64, body: &serde_json::Value) -> Result<(), ApiError> {
        let url = format!("{}/api/v3/providers/proxy/{}/", self.base, pk);
        self.write("PATCH", &url, body.clone())
    }

    /// Deletes a proxy provider.
    pub fn delete_proxy_provider(&self, pk: i64) -> Result<(), ApiError> {
        let url = format!("{}/api/v3/providers/proxy/{}/", self.base, pk);
        self.write("DELETE", &url, serde_json::Value::Null)
    }

    /// Reads the complete application list, following pagination to the end.
    pub fn applications(&self) -> Result<Vec<Application>, ApiError> {
        let mut applications = Vec::new();
        let mut total = None;
        let mut page = 1u64;
        loop {
            // Authentik filters this list by policy for every non-superuser and
            // returns only the applications the account may access. The flag asks
            // for the full list and is honoured only for a superuser — the fleet's
            // provider account is one because it is the sole writer.
            let url = format!(
                "{}/api/v3/core/applications/?page={}&page_size={}&superuser_full_list=true",
                self.base, page, self.page_size
            );
            let body: ApplicationPage = self.get(&url)?;
            total = body.pagination.count.or(total);
            applications.extend(body.results);
            match body.pagination.next {
                Some(next) if next > page => page = next,
                _ => break,
            }
        }

        // The paginator counts before authentik filters the page, so a total above
        // the number of returned objects means the account sees only a subset. That
        // is a configuration error, never a smaller world: acting on it would plan
        // a create for every application the account cannot see (ADR-038).
        if let Some(total) = total {
            let visible = applications.len() as u64;
            if visible < total {
                return Err(ApiError::Config(format!(
                    "the application list is filtered: {visible} of {total} visible; the account must be a superuser or every application must grant it access"
                )));
            }
        }

        Ok(applications)
    }

    /// Reads the complete provider list, for name-to-pk resolution.
    pub fn providers(&self) -> Result<Vec<ProviderSummary>, ApiError> {
        let mut providers = Vec::new();
        let mut page = 1u64;
        loop {
            let url = format!(
                "{}/api/v3/providers/all/?page={}&page_size={}",
                self.base, page, self.page_size
            );
            let body: ProviderPage = self.get(&url)?;
            providers.extend(body.results);
            match body.pagination.next {
                Some(next) if next > page => page = next,
                _ => break,
            }
        }
        Ok(providers)
    }

    /// Reads the OAuth2 providers, filtered by the honest type map.
    ///
    /// The typed endpoint returns every provider (a proxy provider is an
    /// `OAuth2Provider` subclass) and labels them all as OAuth2. The type map
    /// from `/providers/all/` is the only source that tells them apart, so the
    /// caller passes the list it already holds — one snapshot per cycle.
    pub fn oauth2_providers(
        &self,
        types: &[ProviderSummary],
    ) -> Result<Vec<OAuth2Provider>, ApiError> {
        let mut providers = Vec::new();
        let mut page = 1u64;
        loop {
            let url = format!(
                "{}/api/v3/providers/oauth2/?page={}&page_size={}",
                self.base, page, self.page_size
            );
            let body: OAuth2ProviderPage = self.get(&url)?;
            providers.extend(body.results);
            match body.pagination.next {
                Some(next) if next > page => page = next,
                _ => break,
            }
        }
        Ok(providers
            .into_iter()
            .filter(|provider| {
                types
                    .iter()
                    .any(|known| known.pk == provider.pk && known.meta_model_name == OAUTH2_MODEL)
            })
            .collect())
    }

    /// Reads the scope mappings, for managed-scope resolution.
    pub fn scope_mappings(&self) -> Result<Vec<ScopeMapping>, ApiError> {
        let mut mappings = Vec::new();
        let mut page = 1u64;
        loop {
            let url = format!(
                "{}/api/v3/propertymappings/provider/scope/?page={}&page_size={}",
                self.base, page, self.page_size
            );
            let body: ScopeMappingPage = self.get(&url)?;
            mappings.extend(body.results);
            match body.pagination.next {
                Some(next) if next > page => page = next,
                _ => break,
            }
        }
        Ok(mappings)
    }

    /// Creates an OAuth2 provider; `client_secret` is only sent on create.
    pub fn create_oauth2_provider(&self, body: &serde_json::Value) -> Result<(), ApiError> {
        let url = format!("{}/api/v3/providers/oauth2/", self.base);
        self.write("POST", &url, body.clone())
    }

    /// Updates the declared fields of an OAuth2 provider; never the secret.
    pub fn update_oauth2_provider(&self, pk: i64, body: &serde_json::Value) -> Result<(), ApiError> {
        let url = format!("{}/api/v3/providers/oauth2/{}/", self.base, pk);
        self.write("PATCH", &url, body.clone())
    }

    /// Deletes an OAuth2 provider.
    pub fn delete_oauth2_provider(&self, pk: i64) -> Result<(), ApiError> {
        let url = format!("{}/api/v3/providers/oauth2/{}/", self.base, pk);
        self.write("DELETE", &url, serde_json::Value::Null)
    }

    /// Reads the complete binding list, for access-group reconciliation.
    ///
    /// One paginated call, not one per application: a binding carries its
    /// target, so the caller can group them without N+1 requests.
    pub fn bindings(&self) -> Result<Vec<Binding>, ApiError> {
        let mut bindings = Vec::new();
        let mut page = 1u64;
        loop {
            let url = format!(
                "{}/api/v3/policies/bindings/?page={}&page_size={}",
                self.base, page, self.page_size
            );
            let body: BindingPage = self.get(&url)?;
            bindings.extend(body.results);
            match body.pagination.next {
                Some(next) if next > page => page = next,
                _ => break,
            }
        }
        Ok(bindings)
    }

    /// Grants a group access to an application; returns the created binding.
    pub fn create_binding(&self, target: &str, group: &str) -> Result<Binding, ApiError> {
        let url = format!("{}/api/v3/policies/bindings/", self.base);
        self.write_json(
            "POST",
            &url,
            serde_json::json!({
                "target": target,
                "group": group,
                "order": 0,
                "negate": false,
                "enabled": true,
            }),
        )
    }

    /// Removes one binding.
    pub fn delete_binding(&self, pk: &str) -> Result<(), ApiError> {
        let url = format!("{}/api/v3/policies/bindings/{}/", self.base, pk);
        self.write("DELETE", &url, serde_json::Value::Null)
    }

    /// Creates an application and returns the object, whose `pk` is the target
    /// the bindings need.
    pub fn create_application(&self, body: &serde_json::Value) -> Result<Application, ApiError> {
        let url = format!("{}/api/v3/core/applications/", self.base);
        self.write_json("POST", &url, body.clone())
    }

    /// Updates the declared fields of an application, addressed by slug.
    pub fn update_application(&self, slug: &str, body: &serde_json::Value) -> Result<(), ApiError> {
        let url = format!("{}/api/v3/core/applications/{}/", self.base, slug);
        self.write("PATCH", &url, body.clone())
    }

    /// Deletes an application, addressed by slug.
    pub fn delete_application(&self, slug: &str) -> Result<(), ApiError> {
        let url = format!("{}/api/v3/core/applications/{}/", self.base, slug);
        self.write("DELETE", &url, serde_json::Value::Null)
    }

    /// Writes the group's `attributes` object; the caller merges the marker.
    ///
    /// Adoption changes nothing else: `is_superuser`, the description and every
    /// other attribute stay as the API returned them.
    pub fn patch_group(&self, pk: &str, attributes: &serde_json::Value) -> Result<(), ApiError> {
        let url = format!("{}/api/v3/core/groups/{}/", self.base, pk);
        self.write(
            "PATCH",
            &url,
            serde_json::json!({ "attributes": attributes }),
        )
    }

    /// Creates a group with the declared fields; undeclared ones keep the API default.
    pub fn create_group(
        &self,
        name: &str,
        is_superuser: Option<bool>,
        attributes: &serde_json::Value,
        users: Option<&[i64]>,
    ) -> Result<(), ApiError> {
        let url = format!("{}/api/v3/core/groups/", self.base);
        let mut body = serde_json::json!({ "name": name, "attributes": attributes });
        if let Some(flag) = is_superuser {
            body["is_superuser"] = serde_json::json!(flag);
        }
        if let Some(users) = users {
            body["users"] = serde_json::json!(users);
        }
        self.write("POST", &url, body)
    }

    /// Updates the declared fields of a group; an undeclared membership is not sent.
    pub fn update_group(
        &self,
        pk: &str,
        is_superuser: Option<bool>,
        attributes: &serde_json::Value,
        users: Option<&[i64]>,
    ) -> Result<(), ApiError> {
        let url = format!("{}/api/v3/core/groups/{}/", self.base, pk);
        let mut body = serde_json::json!({ "attributes": attributes });
        if let Some(flag) = is_superuser {
            body["is_superuser"] = serde_json::json!(flag);
        }
        if let Some(users) = users {
            body["users"] = serde_json::json!(users);
        }
        self.write("PATCH", &url, body)
    }

    /// Deletes a group.
    pub fn delete_group(&self, pk: &str) -> Result<(), ApiError> {
        let url = format!("{}/api/v3/core/groups/{}/", self.base, pk);
        self.write("DELETE", &url, serde_json::Value::Null)
    }

    /// Writes the user's `attributes` object; the caller merges the marker.
    pub fn patch_user(&self, pk: i64, attributes: &serde_json::Value) -> Result<(), ApiError> {
        let url = format!("{}/api/v3/core/users/{}/", self.base, pk);
        self.write(
            "PATCH",
            &url,
            serde_json::json!({ "attributes": attributes }),
        )
    }

    /// Creates a user with the seed profile and the ownership marker.
    pub fn create_user(
        &self,
        username: &str,
        name: &str,
        email: &str,
        attributes: &serde_json::Value,
    ) -> Result<User, ApiError> {
        let url = format!("{}/api/v3/core/users/", self.base);
        self.write_json(
            "POST",
            &url,
            serde_json::json!({
                "username": username,
                "name": name,
                "email": email,
                "attributes": attributes,
                "is_active": true,
            }),
        )
    }

    /// Sends one write request and maps its failure.
    fn send(
        &self,
        method: &str,
        url: &str,
        body: serde_json::Value,
    ) -> Result<ureq::Response, ApiError> {
        let request = self
            .agent
            .request(method, url)
            .set("Authorization", &format!("Bearer {}", self.token))
            .set("Accept", "application/json");
        let response = if body.is_null() {
            request.call()
        } else {
            request.send_json(body)
        };

        match response {
            Ok(response) => Ok(response),
            Err(ureq::Error::Status(code, response)) => Err(match code {
                401 | 403 => ApiError::Auth,
                429 => ApiError::RateLimited(retry_after(&response)),
                500..=599 => ApiError::Transient(format!("HTTP {code}")),
                _ => ApiError::Config(format!("HTTP {code}")),
            }),
            Err(ureq::Error::Transport(error)) => Err(ApiError::Transient(error.to_string())),
        }
    }

    /// Sends one write request and ignores the response body.
    fn write(&self, method: &str, url: &str, body: serde_json::Value) -> Result<(), ApiError> {
        self.send(method, url, body).map(|_| ())
    }

    /// Sends one write request and reads the created or updated object.
    fn write_json<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        url: &str,
        body: serde_json::Value,
    ) -> Result<T, ApiError> {
        self.send(method, url, body)?
            .into_json()
            .map_err(|error| ApiError::Config(format!("cannot read the response: {error}")))
    }

    fn get<T: serde::de::DeserializeOwned>(&self, url: &str) -> Result<T, ApiError> {
        let response = self
            .agent
            .get(url)
            .set("Authorization", &format!("Bearer {}", self.token))
            .set("Accept", "application/json")
            .call();

        let response = match response {
            Ok(response) => response,
            Err(ureq::Error::Status(code, response)) => {
                return Err(match code {
                    401 | 403 => ApiError::Auth,
                    429 => ApiError::RateLimited(retry_after(&response)),
                    500..=599 => ApiError::Transient(format!("HTTP {code}")),
                    _ => ApiError::Config(format!("HTTP {code}")),
                });
            }
            Err(ureq::Error::Transport(error)) => {
                return Err(ApiError::Transient(error.to_string()));
            }
        };

        response
            .into_json()
            .map_err(|error| ApiError::Transient(format!("invalid JSON: {error}")))
    }
}

/// Reads the `Retry-After` hint, when the server sent one.
fn retry_after(response: &ureq::Response) -> Option<u64> {
    response
        .header("Retry-After")
        .and_then(|value| value.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    /// Serves one HTTP response per connection, in order, then stops.
    fn server(responses: Vec<(u16, &'static str)>) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            for (status, body) in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buffer = [0u8; 4096];
                let _ = stream.read(&mut buffer);
                let reason = match status {
                    200 => "OK",
                    401 => "Unauthorized",
                    429 => "Too Many Requests",
                    500 => "Internal Server Error",
                    _ => "Status",
                };
                let response = format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).unwrap();
            }
        });
        (format!("http://{address}"), handle)
    }

    fn client(base: &str) -> Authentik {
        Authentik::new(base, "token".to_string(), 1, Duration::from_secs(5))
    }

    /// Reads a full HTTP request, including a body split across TCP chunks.
    fn read_request(stream: &mut std::net::TcpStream) -> String {
        let mut data = Vec::new();
        let mut buffer = [0u8; 4096];
        loop {
            let read = stream.read(&mut buffer).unwrap();
            if read == 0 {
                break;
            }
            data.extend_from_slice(&buffer[..read]);
            let Some(position) = data.windows(4).position(|window| window == b"\r\n\r\n") else {
                continue;
            };
            let header_end = position + 4;
            let headers = String::from_utf8_lossy(&data[..header_end]).to_lowercase();
            let length = headers
                .lines()
                .find_map(|line| line.strip_prefix("content-length:"))
                .and_then(|value| value.trim().parse::<usize>().ok())
                .unwrap_or(0);
            if data.len() >= header_end + length {
                break;
            }
        }
        String::from_utf8_lossy(&data).to_string()
    }

    /// Serves one PATCH, returning the raw request so the body can be checked.
    fn patch_server() -> (String, thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_request(&mut stream);
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
                )
                .unwrap();
            request
        });
        (format!("http://{address}"), handle)
    }

    #[test]
    fn pagination_follows_next_until_null() {
        let (base, handle) = server(vec![
            (
                200,
                r#"{"pagination":{"next":2},"results":[{"pk":"1","name":"a","is_superuser":false}]}"#,
            ),
            (
                200,
                r#"{"pagination":{"next":null},"results":[{"pk":"2","name":"b","is_superuser":true}]}"#,
            ),
        ]);

        let groups = client(&base).groups().expect("both pages should parse");

        assert_eq!(groups.len(), 2);
        assert_eq!(groups[1].name, "b");
        assert!(groups[1].is_superuser);
        handle.join().unwrap();
    }

    #[test]
    fn a_filtered_application_list_is_refused() {
        let (base, handle) = server(vec![(
            200,
            r#"{"pagination":{"count":22,"next":null},"results":[{"pk":"pbm-esphome","slug":"esphome"},{"pk":"pbm-ldap","slug":"ldap"}]}"#,
        )]);

        let error = client(&base).applications().unwrap_err();

        assert!(
            error.message().contains("filtered"),
            "the total above the visible count must be loud: {}",
            error.message()
        );
        handle.join().unwrap();
    }

    #[test]
    fn an_unfiltered_application_list_is_accepted() {
        let (base, handle) = server(vec![(
            200,
            r#"{"pagination":{"count":2,"next":null},"results":[{"pk":"pbm-esphome","slug":"esphome"},{"pk":"pbm-ldap","slug":"ldap"}]}"#,
        )]);

        let applications = client(&base).applications().expect("count matches");

        assert_eq!(applications.len(), 2);
        handle.join().unwrap();
    }

    #[test]
    fn a_401_is_an_auth_error() {
        let (base, handle) = server(vec![(401, "{}")]);
        assert!(matches!(client(&base).groups(), Err(ApiError::Auth)));
        handle.join().unwrap();
    }

    #[test]
    fn a_429_is_rate_limited() {
        let (base, handle) = server(vec![(429, "{}")]);
        assert!(matches!(
            client(&base).groups(),
            Err(ApiError::RateLimited(None))
        ));
        handle.join().unwrap();
    }

    #[test]
    fn a_500_is_transient() {
        let (base, handle) = server(vec![(500, "{}")]);
        assert!(matches!(
            client(&base).groups(),
            Err(ApiError::Transient(_))
        ));
        handle.join().unwrap();
    }

    #[test]
    fn patching_a_group_sends_the_attributes() {
        let (base, handle) = patch_server();
        client(&base)
            .patch_group("group-uuid", &serde_json::json!({ "vyrx": "owned" }))
            .expect("the PATCH should be accepted");

        let request = handle.join().unwrap();
        assert!(request.starts_with("PATCH /api/v3/core/groups/group-uuid/ "));
        let body = request
            .split("\r\n\r\n")
            .nth(1)
            .expect("the body is present");
        let body: serde_json::Value = serde_json::from_str(body).expect("the body is JSON");
        assert_eq!(body["attributes"]["vyrx"], serde_json::json!("owned"));
    }

    #[test]
    fn oauth2_providers_drop_the_proxy_subclass() {
        let (base, handle) = server(vec![
            (
                200,
                r#"{"pagination":{"next":null},"results":[{"pk":1,"name":"proxy","meta_model_name":"authentik_providers_proxy.proxyprovider"},{"pk":2,"name":"oauth","meta_model_name":"authentik_providers_oauth2.oauth2provider"}]}"#,
            ),
            (
                200,
                r#"{"pagination":{"next":null},"results":[{"pk":1,"name":"proxy","client_id":"a"},{"pk":2,"name":"oauth","client_id":"b"}]}"#,
            ),
        ]);

        let client = client(&base);
        let types = client.providers().expect("the type map parses");
        let providers = client
            .oauth2_providers(&types)
            .expect("both lists parse");

        assert_eq!(providers.len(), 1, "a proxy row is not an OAuth2 provider");
        assert_eq!(providers[0].name, "oauth");
        handle.join().unwrap();
    }
}
