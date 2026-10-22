//! Cloudflare API client: the only place in this package that does I/O.
//!
//! Deliberately thin. The rules live in [`crate::model`]; this module only
//! translates them into HTTP and back.

use serde_json::{json, Value};

use crate::model::DnsRecord;

/// API failures, mapped to the conditions the protocol defines.
#[derive(Debug)]
pub enum CfError {
    /// Credentials were rejected.
    Auth(String),
    /// The API asked us to slow down.
    RateLimited(String),
    /// Anything transient: 5xx, transport, parsing.
    Transient(String),
}

impl CfError {
    /// Protocol condition kind for this error.
    pub fn condition_kind(&self) -> &'static str {
        match self {
            CfError::Auth(_) => "auth",
            CfError::RateLimited(_) => "rate-limited",
            CfError::Transient(_) => "transient",
        }
    }

    /// Human-readable detail.
    pub fn detail(&self) -> &str {
        match self {
            CfError::Auth(detail) | CfError::RateLimited(detail) | CfError::Transient(detail) => {
                detail
            }
        }
    }
}

/// A Cloudflare client for one token.
pub struct Cloudflare {
    token: String,
}

impl Cloudflare {
    /// Builds a client for `token`.
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            token: token.into(),
        }
    }

    /// One API call; `body` is sent as JSON when present.
    fn call(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value, CfError> {
        let url = format!("https://api.cloudflare.com/client/v4{path}");
        let request = ureq::request(method, &url)
            .set("Authorization", &format!("Bearer {}", self.token))
            .set("User-Agent", "nod-provider-cloudflare/0.1.0");

        let result = match body {
            Some(body) => request.send_json(body),
            None => request.call(),
        };

        match result {
            Ok(response) => response
                .into_json::<Value>()
                .map_err(|error| CfError::Transient(format!("invalid JSON from API: {error}"))),
            Err(ureq::Error::Status(code, response)) => {
                let detail = response
                    .into_json::<Value>()
                    .ok()
                    .and_then(|value| {
                        value
                            .get("errors")
                            .and_then(|errors| errors.get(0))
                            .and_then(|first| first.get("message"))
                            .and_then(|message| message.as_str())
                            .map(str::to_string)
                    })
                    .unwrap_or_else(|| format!("HTTP {code}"));
                match code {
                    401 | 403 => Err(CfError::Auth(detail)),
                    429 => Err(CfError::RateLimited(detail)),
                    _ => Err(CfError::Transient(detail)),
                }
            }
            Err(ureq::Error::Transport(transport)) => {
                Err(CfError::Transient(transport.to_string()))
            }
        }
    }

    /// Resolves a zone name to its id.
    pub fn zone_id(&self, zone: &str) -> Result<String, CfError> {
        let value = self.call("GET", &format!("/zones?name={zone}"), None)?;
        value
            .get("result")
            .and_then(|result| result.get(0))
            .and_then(|zone| zone.get("id"))
            .and_then(|id| id.as_str())
            .map(str::to_string)
            .ok_or_else(|| CfError::Transient(format!("zone {zone} not found")))
    }

    /// Lists every DNS record in a zone, following pagination.
    pub fn list_records(&self, zone_id: &str) -> Result<Vec<DnsRecord>, CfError> {
        let mut records = Vec::new();
        let mut page = 1u32;

        loop {
            let value = self.call(
                "GET",
                &format!("/zones/{zone_id}/dns_records?per_page=100&page={page}"),
                None,
            )?;
            let batch: Vec<DnsRecord> = value
                .get("result")
                .cloned()
                .map(serde_json::from_value)
                .transpose()
                .map_err(|error| CfError::Transient(format!("cannot parse records: {error}")))?
                .unwrap_or_default();
            let count = batch.len();
            records.extend(batch);

            let total_pages = value
                .get("result_info")
                .and_then(|info| info.get("total_pages"))
                .and_then(Value::as_u64)
                .unwrap_or(1);
            if count == 0 || u64::from(page) >= total_pages {
                break;
            }
            page += 1;
        }

        Ok(records)
    }

    /// Creates a record and returns its id.
    pub fn create_record(
        &self,
        zone_id: &str,
        record_type: &str,
        name: &str,
        content: &str,
        ttl: u32,
        proxied: bool,
        comment: &str,
    ) -> Result<String, CfError> {
        let value = self.call(
            "POST",
            &format!("/zones/{zone_id}/dns_records"),
            Some(json!({
                "type": record_type,
                "name": name,
                "content": content,
                "ttl": ttl,
                "proxied": proxied,
                "comment": comment,
            })),
        )?;
        value
            .get("result")
            .and_then(|result| result.get("id"))
            .and_then(|id| id.as_str())
            .map(str::to_string)
            .ok_or_else(|| CfError::Transient("create returned no record id".to_string()))
    }

    /// Updates a record in place.
    pub fn update_record(
        &self,
        zone_id: &str,
        record_id: &str,
        record_type: &str,
        name: &str,
        content: &str,
        ttl: u32,
        proxied: bool,
        comment: &str,
    ) -> Result<(), CfError> {
        self.call(
            "PATCH",
            &format!("/zones/{zone_id}/dns_records/{record_id}"),
            Some(json!({
                "type": record_type,
                "name": name,
                "content": content,
                "ttl": ttl,
                "proxied": proxied,
                "comment": comment,
            })),
        )?;
        Ok(())
    }

    /// Deletes a record.
    pub fn delete_record(&self, zone_id: &str, record_id: &str) -> Result<(), CfError> {
        self.call(
            "DELETE",
            &format!("/zones/{zone_id}/dns_records/{record_id}"),
            None,
        )?;
        Ok(())
    }
}
