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
}
