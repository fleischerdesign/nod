//! End-to-end protocol test: no token, no Authentik, just a fake API.
//!
//! Other nod users can run `cargo test` and watch the provider speak the
//! contract; no fleet wiring, contract or Nix path is involved.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::process::{Command, Stdio};
use std::thread;

/// Serves one page per endpoint the provider reads: groups, users, applications.
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
            // users, applications, proxy providers, flows, providers, outposts
            r#"{"pagination":{"count":0,"next":null},"results":[]}"#,
            r#"{"pagination":{"count":0,"next":null},"results":[]}"#,
            r#"{"pagination":{"count":0,"next":null},"results":[]}"#,
            r#"{"pagination":{"count":0,"next":null},"results":[]}"#,
            r#"{"pagination":{"count":0,"next":null},"results":[]}"#,
            r#"{"pagination":{"count":0,"next":null},"results":[]}"#,
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
    assert_eq!(resources.len(), 2);
    assert_eq!(resources[0]["id"], "admins");
    assert_eq!(resources[0]["owned"], true);
    assert_eq!(resources[1]["owned"], false);

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
