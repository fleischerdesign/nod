//! End-to-end protocol test: no token, no Authentik, just a fake API.
//!
//! Other nod users can run `cargo test` and watch the provider speak the
//! contract; no fleet wiring, contract or Nix path is involved.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::process::{Command, Stdio};
use std::thread;

/// Serves one page per endpoint the provider reads, in the order `observe`
/// requests them: groups, users, applications, proxy providers, flows, the
/// provider type map, outposts, the typed OAuth2 list, scope mappings.
///
/// The OAuth2 row carries a `client_secret` exactly as the API returns it; the
/// provider must not pass it on (ADR-043).
fn fake_authentik() -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        let bodies = [
            // groups
            r#"{"pagination":{"count":2,"next":null},"results":[
                {"pk":"1","name":"admins","is_superuser":true,"attributes":{"vyrx":"owned"}},
                {"pk":"2","name":"media","is_superuser":false,"attributes":{}}
            ]}"#,
            // users
            r#"{"pagination":{"count":0,"next":null},"results":[]}"#,
            // applications
            r#"{"pagination":{"count":0,"next":null},"results":[]}"#,
            // proxy providers
            r#"{"pagination":{"count":0,"next":null},"results":[]}"#,
            // flows
            r#"{"pagination":{"count":0,"next":null},"results":[]}"#,
            // provider type map: one proxy, one OAuth2
            r#"{"pagination":{"count":2,"next":null},"results":[
                {"pk":1,"name":"bazarr","meta_model_name":"authentik_providers_proxy.proxyprovider"},
                {"pk":2,"name":"grafana","meta_model_name":"authentik_providers_oauth2.oauth2provider"}
            ]}"#,
            // outposts
            r#"{"pagination":{"count":0,"next":null},"results":[]}"#,
            // typed OAuth2 list: serializes the proxy subclass as OAuth2 too
            r#"{"pagination":{"count":2,"next":null},"results":[
                {"pk":1,"name":"bazarr","client_id":"proxy-client","client_secret":"proxy-secret"},
                {"pk":2,"name":"grafana","client_id":"grafana-client","client_secret":"super-secret-value","sub_mode":"user_email","include_claims_in_id_token":true,"signing_key":"key-uuid"}
            ]}"#,
            // scope mappings
            r#"{"pagination":{"count":0,"next":null},"results":[]}"#,
            // certificate keypairs: the API is write-only for the material, so a
            // response that carried it anyway must not leak it
            r#"{"pagination":{"count":1,"next":null},"results":[
                {"pk":"key-uuid","name":"nod OIDC signing key","key_type":"rsa","fingerprint_sha256":"ab:cd","key_data":"-----BEGIN PRIVATE KEY-----","certificate_data":"-----BEGIN CERTIFICATE-----"}
            ]}"#,
        ];
        for body in bodies {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0u8; 4096];
            let _ = stream.read(&mut buffer);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).unwrap();
        }
    });
    (format!("http://{address}"), handle)
}

#[test]
fn the_provider_speaks_the_contract_without_a_real_instance() {
    let (base, server) = fake_authentik();
    let mut child = Command::new(env!("CARGO_BIN_EXE_nod-provider-authentik"))
        .env("AUTHENTIK_TOKEN", "test-token")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("provider should start");

    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());

    let mut send = |request: serde_json::Value| -> serde_json::Value {
        writeln!(stdin, "{}", serde_json::to_string(&request).unwrap()).unwrap();
        stdin.flush().unwrap();
        let mut line = String::new();
        stdout.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    };

    let handshake = send(serde_json::json!({ "protocol": 1 }));
    assert_eq!(handshake["provider"], "authentik");
    assert_eq!(handshake["capabilities"][0], "authentik.group");
    assert_eq!(handshake["ownership"], "marker");
    assert!(handshake["capabilities"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!("authentik.provider.oauth2")));

    let observed = send(serde_json::json!({
        "op": "observe",
        "config": {
            "url": base,
            "token_env": "AUTHENTIK_TOKEN",
            "marker_key": "vyrx",
            "marker_value": "owned"
        }
    }));
    let resources = observed["resources"].as_array().unwrap();
    assert_eq!(resources.len(), 4);
    assert_eq!(resources[0]["id"], "admins");
    assert_eq!(resources[0]["owned"], true);
    assert_eq!(resources[1]["owned"], false);

    // The typed list labelled the proxy row as OAuth2; the type map is what
    // keeps it out. The one real OAuth2 provider arrives without its secret.
    let oauth2 = resources
        .iter()
        .find(|resource| resource["kind"] == "authentik.provider.oauth2")
        .expect("the OAuth2 provider is observed");
    assert_eq!(oauth2["id"], "grafana");
    assert_eq!(oauth2["attributes"]["client_id"], "grafana-client");
    assert_eq!(
        oauth2["attributes"]["signing_key"],
        "nod OIDC signing key"
    );
    assert!(oauth2["attributes"].get("client_secret").is_none());
    assert!(
        !observed.to_string().contains("super-secret-value"),
        "the secret must never reach the wire"
    );

    // The keypair is observed as facts; the material never leaves the API.
    let keypair = resources
        .iter()
        .find(|resource| resource["kind"] == "authentik.certificatekeypair")
        .expect("the keypair is observed");
    assert_eq!(keypair["attributes"]["key_type"], "rsa");
    assert!(
        !observed.to_string().contains("BEGIN PRIVATE KEY"),
        "key material must never reach the wire"
    );
    assert!(
        resources
            .iter()
            .all(|resource| resource["id"] != "bazarr"),
        "a proxy provider is not an OAuth2 provider"
    );

    let planned = send(serde_json::json!({
        "op": "plan",
        "config": {},
        "desired": [{
            "kind": "authentik.group",
            "id": "new",
            "attributes": { "is_superuser": false }
        }],
        "observed": resources
    }));
    assert_eq!(planned["changes"][0]["kind"], "create");

    let applied = send(serde_json::json!({
        "op": "apply",
        "config": {},
        "changes": []
    }));
    assert!(applied["applied"].as_array().unwrap().is_empty());

    let adopted = send(serde_json::json!({
        "op": "adopt",
        "config": {},
        "resources": []
    }));
    assert!(adopted["adopted"].as_array().unwrap().is_empty());

    drop(stdin);
    assert!(child.wait().unwrap().success());
    server.join().unwrap();
}
