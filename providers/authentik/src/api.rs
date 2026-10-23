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

/// One page of the Authentik list response.
#[derive(Debug, Default, Deserialize)]
struct Page {
    #[serde(default)]
    results: Vec<Group>,
    #[serde(default)]
    pagination: Pagination,
}

/// Authentik's pagination block.
#[derive(Debug, Default, Deserialize)]
struct Pagination {
    #[serde(default)]
    next: Option<u64>,
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
                "{}/api/v3/core/groups/?page={}&page_size={}",
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

    fn get(&self, url: &str) -> Result<Page, ApiError> {
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

    #[test]
    fn pagination_follows_next_until_null() {
        let (base, handle) = server(vec![
            (
                200,
                r#"{"pagination":{"next":2},"results":[{"name":"a","is_superuser":false}]}"#,
            ),
            (
                200,
                r#"{"pagination":{"next":null},"results":[{"name":"b","is_superuser":true}]}"#,
            ),
        ]);

        let groups = client(&base).groups().expect("both pages should parse");

        assert_eq!(groups.len(), 2);
        assert_eq!(groups[1].name, "b");
        assert!(groups[1].is_superuser);
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
}
