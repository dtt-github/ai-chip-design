//! End-to-end API tests against a real Postgres (one fresh database per test).
//! Run with `DATABASE_URL` pointing at a server where the user may create databases.

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

struct Harness {
    app: Router,
    _dir: tempfile::TempDir,
}

fn harness(db: PgPool) -> Harness {
    let (state, dir) = common::state(db, LspConfig::disabled());
    Harness { app: router(state), _dir: dir }
}

impl Harness {
    async fn call(&self, method: Method, uri: &str, tok: Option<&str>, body: Option<Value>) -> (StatusCode, Value) {
        let mut req = Request::builder().method(method).uri(uri);
        if let Some(t) = tok {
            req = req.header("authorization", format!("Bearer {t}"));
        }
        let req = match body {
            Some(b) => req.header("content-type", "application/json").body(Body::from(b.to_string())),
            None => req.body(Body::empty()),
        }
        .unwrap();
        let res = self.app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        let value = if bytes.is_empty() { Value::Null } else { serde_json::from_slice(&bytes).unwrap() };
        (status, value)
    }
}

#[sqlx::test(migrator = "wafer_api::MIGRATOR")]
async fn auth_required_and_me(db: PgPool) {
    let h = harness(db);
    let (s, _) = h.call(Method::GET, "/healthz", None, None).await;
    assert_eq!(s, StatusCode::OK);
    let (s, b) = h.call(Method::GET, "/api/me", None, None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "{b}");
    let (s, _) = h.call(Method::GET, "/api/me", Some("garbage"), None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);

    let t = token("ada", "ada@example.org");
    let (s, me1) = h.call(Method::GET, "/api/me", Some(&t), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(me1["email"], "ada@example.org");
    assert_eq!(me1["display_name"], "ada");
    // Same subject maps to the same user.
    let (_, me2) = h.call(Method::GET, "/api/me", Some(&t), None).await;
    assert_eq!(me1["id"], me2["id"]);
}

#[sqlx::test(migrator = "wafer_api::MIGRATOR")]
async fn project_lifecycle(db: PgPool) {
    let h = harness(db);
    let ada = token("ada", "ada@example.org");

    let (s, p) = h.call(Method::POST, "/api/projects", Some(&ada), Some(json!({"name": "My Counter"}))).await;
    assert_eq!(s, StatusCode::CREATED, "{p}");
    assert_eq!(p["slug"], "my-counter");
    assert_eq!(p["pdk"], "ihp-sg13g2");
    assert_eq!(p["role"], "owner");
    let id = p["id"].as_str().unwrap().to_string();
    let head = p["head"].as_str().unwrap().to_string();

    let (s, _) = h.call(Method::POST, "/api/projects", Some(&ada), Some(json!({"name": "my counter"}))).await;
    assert_eq!(s, StatusCode::CONFLICT);

    let (_, list) = h.call(Method::GET, "/api/projects", Some(&ada), None).await;
    assert_eq!(list.as_array().unwrap().len(), 1);

    let (s, tree) = h.call(Method::GET, &format!("/api/projects/{id}/tree"), Some(&ada), None).await;
    assert_eq!(s, StatusCode::OK);
    let paths: Vec<&str> = tree["files"].as_array().unwrap().iter().map(|f| f["path"].as_str().unwrap()).collect();
    assert_eq!(paths, vec!["README.md", "config.json", "src/my_counter.v", "tb/tb_my_counter.v"]);

    let (s, snap) = h.call(Method::GET, &format!("/api/projects/{id}/snapshot"), Some(&ada), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(snap["commit"], head.as_str());
    assert!(snap["files"]["src/my_counter.v"].as_str().unwrap().contains("module my_counter"));
    assert_eq!(snap["files"].as_object().unwrap().len(), 4);

    let (s, f) = h.call(Method::GET, &format!("/api/projects/{id}/files/src/my_counter.v"), Some(&ada), None).await;
    assert_eq!(s, StatusCode::OK);
    assert!(f["content"].as_str().unwrap().contains("module my_counter"));
    assert_eq!(f["commit"], head.as_str());

    let commit = json!({
        "message": "Add ALU", "parent": head,
        "changes": [
            {"op": "write", "path": "src/alu.v", "content": "module alu; endmodule\n"},
            {"op": "delete", "path": "README.md"}
        ]
    });
    let (s, c) = h.call(Method::POST, &format!("/api/projects/{id}/commits"), Some(&ada), Some(commit.clone())).await;
    assert_eq!(s, StatusCode::CREATED, "{c}");
    assert_eq!(c["author_email"], "ada@example.org");

    // Same parent again: someone else already moved the branch.
    let (s, _) = h.call(Method::POST, &format!("/api/projects/{id}/commits"), Some(&ada), Some(commit)).await;
    assert_eq!(s, StatusCode::CONFLICT);

    let (_, log) = h.call(Method::GET, &format!("/api/projects/{id}/commits"), Some(&ada), None).await;
    let msgs: Vec<&str> = log.as_array().unwrap().iter().map(|c| c["message"].as_str().unwrap()).collect();
    assert_eq!(msgs, vec!["Add ALU", "Initial commit"]);

    let bad = json!({"message": "x", "changes": [{"op": "write", "path": "../etc/passwd", "content": ""}]});
    let (s, _) = h.call(Method::POST, &format!("/api/projects/{id}/commits"), Some(&ada), Some(bad)).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

#[sqlx::test(migrator = "wafer_api::MIGRATOR")]
async fn access_control(db: PgPool) {
    let h = harness(db);
    let ada = token("ada", "ada@example.org");
    let bob = token("bob", "bob@example.org");
    let (_, p) = h.call(Method::POST, "/api/projects", Some(&ada), Some(json!({"name": "secret"}))).await;
    let id = p["id"].as_str().unwrap().to_string();
    let commit = json!({"message": "m", "changes": [{"op": "write", "path": "x.v", "content": "1"}]});

    // Private project is invisible to non-members.
    let (s, _) = h.call(Method::GET, &format!("/api/projects/{id}"), Some(&bob), None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    // Users must have signed in once before they can be added (Bob did, above).
    let (s, _) = h
        .call(
            Method::POST,
            &format!("/api/projects/{id}/members"),
            Some(&ada),
            Some(json!({"email": "carol@example.org", "role": "viewer"})),
        )
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, m) = h
        .call(
            Method::POST,
            &format!("/api/projects/{id}/members"),
            Some(&ada),
            Some(json!({"email": "BOB@example.org", "role": "viewer"})),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{m}");

    // Viewer can read but not write or manage members.
    let (s, _) = h.call(Method::GET, &format!("/api/projects/{id}/tree"), Some(&bob), None).await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = h.call(Method::POST, &format!("/api/projects/{id}/commits"), Some(&bob), Some(commit.clone())).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = h
        .call(
            Method::POST,
            &format!("/api/projects/{id}/members"),
            Some(&bob),
            Some(json!({"email": "bob@example.org", "role": "editor"})),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    // Promote to editor: now commits work and are attributed to Bob.
    h.call(
        Method::POST,
        &format!("/api/projects/{id}/members"),
        Some(&ada),
        Some(json!({"email": "bob@example.org", "role": "editor"})),
    )
    .await;
    let (s, c) = h.call(Method::POST, &format!("/api/projects/{id}/commits"), Some(&bob), Some(commit)).await;
    assert_eq!(s, StatusCode::CREATED);
    assert_eq!(c["author_email"], "bob@example.org");

    let (_, members) = h.call(Method::GET, &format!("/api/projects/{id}/members"), Some(&bob), None).await;
    assert_eq!(members.as_array().unwrap().len(), 2);
}

#[sqlx::test(migrator = "wafer_api::MIGRATOR")]
async fn public_projects_are_readable(db: PgPool) {
    let h = harness(db);
    let ada = token("ada", "ada@example.org");
    let eve = token("eve", "eve@example.org");
    let (_, p) = h
        .call(Method::POST, "/api/projects", Some(&ada), Some(json!({"name": "open core", "visibility": "public"})))
        .await;
    let id = p["id"].as_str().unwrap();
    let (s, v) = h.call(Method::GET, &format!("/api/projects/{id}"), Some(&eve), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["role"], Value::Null);
    let commit = json!({"message": "m", "changes": [{"op": "write", "path": "x.v", "content": "1"}]});
    let (s, _) = h.call(Method::POST, &format!("/api/projects/{id}/commits"), Some(&eve), Some(commit)).await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    let (s, _) =
        h.call(Method::POST, "/api/projects", Some(&ada), Some(json!({"name": "x", "visibility": "everyone"}))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}
