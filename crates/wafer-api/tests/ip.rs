//! IP registry: publish, list, licence checks, import pinned to a commit, upgrade.

mod common;

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use common::token;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use wafer_api::lsp::LspConfig;
use wafer_api::router;

struct H {
    app: Router,
    _dir: tempfile::TempDir,
}

impl H {
    async fn call(&self, method: Method, uri: &str, tok: &str, body: Option<Value>) -> (StatusCode, Value) {
        let req = Request::builder().method(method).uri(uri).header("authorization", format!("Bearer {tok}"));
        let req = match body {
            Some(b) => req.header("content-type", "application/json").body(Body::from(b.to_string())),
            None => req.body(Body::empty()),
        }
        .unwrap();
        let res = self.app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (status, if bytes.is_empty() { Value::Null } else { serde_json::from_slice(&bytes).unwrap() })
    }

    async fn project(&self, tok: &str, body: Value) -> String {
        let (s, p) = self.call(Method::POST, "/api/projects", tok, Some(body)).await;
        assert_eq!(s, StatusCode::CREATED, "{p}");
        p["id"].as_str().unwrap().to_string()
    }

    async fn file(&self, tok: &str, project: &str, path: &str) -> String {
        let (s, f) = self.call(Method::GET, &format!("/api/projects/{project}/files/{path}"), tok, None).await;
        assert_eq!(s, StatusCode::OK, "{path}: {f}");
        f["content"].as_str().unwrap().to_string()
    }

    async fn write(&self, tok: &str, project: &str, path: &str, content: &str) {
        let body = json!({ "message": "edit", "changes": [{ "op": "write", "path": path, "content": content }] });
        let (s, b) = self.call(Method::POST, &format!("/api/projects/{project}/commits"), tok, Some(body)).await;
        assert_eq!(s, StatusCode::CREATED, "{b}");
    }
}

#[sqlx::test(migrator = "wafer_api::MIGRATOR")]
async fn publish_check_import_upgrade(db: PgPool) {
    let (state, dir) = common::state(db, LspConfig::disabled());
    let h = H { app: router(state), _dir: dir };
    let ada = token("ada", "ada@example.org");
    let bob = token("bob", "bob@example.org");

    // Publish the template counter from a public Apache-2.0 project.
    let counter = h.project(&ada, json!({ "name": "Counter", "visibility": "public" })).await;
    let (s, v) = h
        .call(
            Method::POST,
            &format!("/api/projects/{counter}/ip"),
            &ada,
            Some(json!({ "version": "1.0.0", "description": "8-bit counter" })),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    assert_eq!(v["block"], "counter");
    assert_eq!(v["licence"], "Apache-2.0");
    assert_eq!(v["files"].as_object().unwrap().keys().collect::<Vec<_>>(), vec!["src/counter.v"]);
    assert_eq!(v["params"]["WIDTH"], 8);
    assert!(v["ports"].as_array().unwrap().contains(&json!({ "name": "count", "dir": "output", "width": 8 })), "{v}");
    assert_eq!(v["gate"]["passed"], true);
    let pinned = v["commit_id"].as_str().unwrap().to_string();
    for bad in ["1.0.0", "0.9.0", "1.0"] {
        let (s, _) =
            h.call(Method::POST, &format!("/api/projects/{counter}/ip"), &ada, Some(json!({ "version": bad }))).await;
        assert!(s == StatusCode::CONFLICT || s == StatusCode::BAD_REQUEST, "{bad}: {s}");
    }
    // Only the owning project may publish under the name; editors only.
    let other = h.project(&bob, json!({ "name": "Other" })).await;
    let (s, _) = h
        .call(
            Method::POST,
            &format!("/api/projects/{other}/ip"),
            &bob,
            Some(json!({ "name": "counter", "version": "2.0.0" })),
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT);
    let (s, _) =
        h.call(Method::POST, &format!("/api/projects/{counter}/ip"), &bob, Some(json!({ "version": "2.0.0" }))).await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    // Bob sees it in the registry.
    let (_, list) = h.call(Method::GET, "/api/ip", &bob, None).await;
    assert_eq!(list[0]["name"], "counter");
    assert_eq!(list[0]["project_name"], "Counter");
    assert_eq!(list[0]["versions"][0]["version"], "1.0.0");

    // Apache-2.0 into a GPL-2.0-only project: refused.
    let gpl = h.project(&bob, json!({ "name": "Gpl soc", "licence": "GPL-2.0-only" })).await;
    let (s, c) =
        h.call(Method::GET, &format!("/api/projects/{gpl}/ip/check?block=counter&version=1.0.0"), &bob, None).await;
    assert_eq!(s, StatusCode::OK, "{c}");
    assert_eq!(c["licence"]["status"], "incompatible");
    assert_eq!(c["allowed"], false);
    let (s, e) = h
        .call(
            Method::POST,
            &format!("/api/projects/{gpl}/ip/import"),
            &bob,
            Some(json!({ "block": "counter", "version": "1.0.0" })),
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT, "{e}");
    let (s, _) = h.call(Method::POST, "/api/projects", &bob, Some(json!({ "name": "x", "licence": "Beerware" }))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // Into an Apache-2.0 project: imported, pinned, added to the flow config.
    let soc = h.project(&bob, json!({ "name": "Soc" })).await;
    let (_, c) =
        h.call(Method::GET, &format!("/api/projects/{soc}/ip/check?block=counter&version=1.0.0"), &bob, None).await;
    assert_eq!(c["licence"]["status"], "compatible", "{c}");
    assert_eq!(c["allowed"], true);
    assert_eq!(c["files"], json!(["ip/counter/src/counter.v"]));
    assert_eq!(c["configs"], json!(["config.json"]));
    let (s, r) = h
        .call(
            Method::POST,
            &format!("/api/projects/{soc}/ip/import"),
            &bob,
            Some(json!({ "block": "counter", "version": "1.0.0" })),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{r}");
    assert_eq!(r["commit"], pinned.as_str());
    let (_, log) = h.call(Method::GET, &format!("/api/projects/{soc}/commits"), &bob, None).await;
    assert_eq!(r["imported_commit"], log[0]["id"]);
    assert_eq!(h.file(&bob, &soc, "ip/counter/src/counter.v").await, h.file(&ada, &counter, "src/counter.v").await);
    let cfg: Value = serde_json::from_str(&h.file(&bob, &soc, "config.json").await).unwrap();
    assert_eq!(cfg["VERILOG_FILES"], json!(["dir::src/soc.v", "dir::ip/counter/src/counter.v"]));
    let (_, log) = h.call(Method::GET, &format!("/api/projects/{soc}/commits"), &bob, None).await;
    let msg = log[0]["message"].as_str().unwrap();
    assert!(msg.starts_with("Import IP block counter 1.0.0\n\nFrom project \"Counter\" at commit"), "{msg}");
    assert!(msg.contains("Licence: Apache-2.0 (compatible with this project's Apache-2.0)"), "{msg}");
    let (_, imports) = h.call(Method::GET, &format!("/api/projects/{soc}/ip/imports"), &bob, None).await;
    assert_eq!(imports["blocks"]["counter"]["version"], "1.0.0");
    assert_eq!(imports["blocks"]["counter"]["commit"], pinned.as_str());
    assert_eq!(imports["blocks"]["counter"]["licence_verdict"]["status"], "compatible");
    let (s, _) = h
        .call(
            Method::POST,
            &format!("/api/projects/{soc}/ip/import"),
            &bob,
            Some(json!({ "block": "counter", "version": "1.0.0" })),
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT, "already imported");

    // A project with its own `counter` module can't import the block.
    let clash = h.project(&bob, json!({ "name": "Counter" })).await;
    let (_, c) =
        h.call(Method::GET, &format!("/api/projects/{clash}/ip/check?block=counter&version=1.0.0"), &bob, None).await;
    assert_eq!(c["allowed"], false);
    assert!(c["collisions"][0].as_str().unwrap().contains("module counter"), "{c}");

    // Upgrade: a new version replaces the old files and pin.
    let rtl = h.file(&ada, &counter, "src/counter.v").await;
    h.write(&ada, &counter, "src/counter.v", &rtl.replace("`default_nettype none", "`default_nettype none\n// v1.1"))
        .await;
    let (s, v2) =
        h.call(Method::POST, &format!("/api/projects/{counter}/ip"), &ada, Some(json!({ "version": "1.1.0" }))).await;
    assert_eq!(s, StatusCode::CREATED, "{v2}");
    let (s, r) = h
        .call(
            Method::POST,
            &format!("/api/projects/{soc}/ip/import"),
            &bob,
            Some(json!({ "block": "counter", "version": "1.1.0" })),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{r}");
    assert_eq!(r["replaces"], "1.0.0");
    assert!(h.file(&bob, &soc, "ip/counter/src/counter.v").await.contains("// v1.1"));
    let cfg: Value = serde_json::from_str(&h.file(&bob, &soc, "config.json").await).unwrap();
    assert_eq!(cfg["VERILOG_FILES"].as_array().unwrap().len(), 2, "{cfg}");
    let (_, log) = h.call(Method::GET, &format!("/api/projects/{soc}/commits"), &bob, None).await;
    assert!(log[0]["message"].as_str().unwrap().starts_with("Update IP block counter 1.0.0 -> 1.1.0"));

    // A licence the check doesn't know: only the owner, and only when accepting it explicitly.
    let odd = h.project(&ada, json!({ "name": "Odd", "visibility": "public" })).await;
    let rtl = h.file(&ada, &odd, "src/odd.v").await;
    h.write(&ada, &odd, "src/odd.v", &rtl.replace("Apache-2.0", "LicenseRef-Acme")).await;
    let (s, v) =
        h.call(Method::POST, &format!("/api/projects/{odd}/ip"), &ada, Some(json!({ "version": "0.1.0" }))).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    let (_, c) =
        h.call(Method::GET, &format!("/api/projects/{soc}/ip/check?block=odd&version=0.1.0"), &bob, None).await;
    assert_eq!(c["licence"]["status"], "unknown");
    assert_eq!((c["allowed"].as_bool(), c["can_accept"].as_bool()), (Some(true), Some(true)));
    let url = format!("/api/projects/{soc}/ip/import");
    let (s, _) = h.call(Method::POST, &url, &bob, Some(json!({ "block": "odd", "version": "0.1.0" }))).await;
    assert_eq!(s, StatusCode::CONFLICT);
    let (s, r) = h
        .call(Method::POST, &url, &bob, Some(json!({ "block": "odd", "version": "0.1.0", "accept_licence": true })))
        .await;
    assert_eq!(s, StatusCode::CREATED, "{r}");
    let lock: Value = serde_json::from_str(&h.file(&bob, &soc, "ip.lock.json").await).unwrap();
    assert_eq!(lock["blocks"]["odd"]["licence_accepted_by"], "bob@example.org");
    assert_eq!(lock["blocks"]["counter"]["version"], "1.1.0");

    // Private blocks stay private; a project can't import itself.
    let secret = h.project(&ada, json!({ "name": "Secret" })).await;
    let (s, _) =
        h.call(Method::POST, &format!("/api/projects/{secret}/ip"), &ada, Some(json!({ "version": "1.0.0" }))).await;
    assert_eq!(s, StatusCode::CREATED);
    let (_, list) = h.call(Method::GET, "/api/ip", &bob, None).await;
    assert!(list.as_array().unwrap().iter().all(|b| b["name"] != "secret"), "{list}");
    let (s, _) =
        h.call(Method::GET, &format!("/api/projects/{soc}/ip/check?block=secret&version=1.0.0"), &bob, None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, _) =
        h.call(Method::GET, &format!("/api/projects/{counter}/ip/check?block=counter&version=1.1.0"), &ada, None).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}
