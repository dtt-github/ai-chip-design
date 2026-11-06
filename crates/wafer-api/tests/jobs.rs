//! Job queue, workers and endpoints, with a fake backend instead of Docker.
//! The design name in config.json picks the fake's behaviour.

mod common;

use std::sync::Arc;
use std::time::Duration;

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
use wafer_jobs::{Backend, BoxFuture, CancellationToken, JobError, JobSpec, Outcome, final_dir};

struct FakeBackend;

impl Backend for FakeBackend {
    fn toolchain(&self) -> String {
        "fake@sha256:0".into()
    }

    fn run<'a>(&'a self, spec: &'a JobSpec, cancel: CancellationToken) -> BoxFuture<'a, Result<Outcome, JobError>> {
        Box::pin(async move {
            let config: Value = serde_json::from_slice(&std::fs::read(spec.workdir.join(&spec.config))?).unwrap();
            let design = config["DESIGN_NAME"].as_str().unwrap().to_string();
            std::fs::write(&spec.log_path, format!("[INFO] starting {design}\n"))?;
            match design.as_str() {
                "fail" => {
                    std::fs::write(&spec.log_path, "[INFO] go\n[12:00:00] ERROR    boom in placement\n")?;
                    Ok(Outcome::Exited(1))
                }
                "slow" => {
                    cancel.cancelled().await;
                    Ok(Outcome::Canceled)
                }
                _ => {
                    let fin = final_dir(&spec.workdir);
                    std::fs::create_dir_all(fin.join("gds"))?;
                    std::fs::write(fin.join(format!("gds/{design}.gds")), b"GDSII")?;
                    std::fs::write(
                        fin.join("metrics.json"),
                        json!({
                            "design__instance__count__stdcell": 56, "timing__setup__ws": 13.7, "timing__hold__ws": 0.2,
                            "magic__drc_error__count": 0, "klayout__drc_error__count": 0,
                            "design__lvs_error__count": 0, "antenna__violating__nets": 0
                        })
                        .to_string(),
                    )?;
                    std::fs::write(&spec.log_path, "[INFO] starting\nFlow complete.\n")?;
                    Ok(Outcome::Exited(0))
                }
            }
        })
    }
}

struct Harness {
    app: Router,
    _dir: tempfile::TempDir,
}

async fn harness(db: PgPool, backend: bool) -> Harness {
    let backend: Option<Arc<dyn Backend>> = backend.then(|| Arc::new(FakeBackend) as Arc<dyn Backend>);
    let (state, dir) = common::state_with_backend(db, LspConfig::disabled(), backend);
    wafer_api::jobs::start_workers(state.clone(), 2).await.unwrap();
    Harness { app: router(state), _dir: dir }
}

impl Harness {
    async fn raw(&self, method: Method, uri: &str, tok: &str, body: Option<Value>) -> (StatusCode, Vec<u8>) {
        let req = Request::builder().method(method).uri(uri).header("authorization", format!("Bearer {tok}"));
        let req = match body {
            Some(b) => req.header("content-type", "application/json").body(Body::from(b.to_string())),
            None => req.body(Body::empty()),
        }
        .unwrap();
        let res = self.app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        (status, res.into_body().collect().await.unwrap().to_bytes().to_vec())
    }

    async fn call(&self, method: Method, uri: &str, tok: &str, body: Option<Value>) -> (StatusCode, Value) {
        let (status, bytes) = self.raw(method, uri, tok, body).await;
        (status, if bytes.is_empty() { Value::Null } else { serde_json::from_slice(&bytes).unwrap() })
    }

    /// Creates a project whose config.json names `design`.
    async fn project(&self, tok: &str, design: &str) -> String {
        let (_, p) = self.call(Method::POST, "/api/projects", tok, Some(json!({ "name": design }))).await;
        let id = p["id"].as_str().unwrap().to_string();
        let cfg = json!({ "DESIGN_NAME": design, "VERILOG_FILES": "dir::src/*.v" }).to_string();
        let (s, _) = self
            .call(
                Method::POST,
                &format!("/api/projects/{id}/commits"),
                tok,
                Some(
                    json!({ "message": "cfg", "changes": [{ "op": "write", "path": "config.json", "content": cfg }] }),
                ),
            )
            .await;
        assert_eq!(s, StatusCode::CREATED);
        id
    }

    async fn wait(&self, tok: &str, job: &str, until: impl Fn(&str) -> bool) -> Value {
        for _ in 0..100 {
            let (_, j) = self.call(Method::GET, &format!("/api/jobs/{job}"), tok, None).await;
            if until(j["status"].as_str().unwrap()) {
                return j;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("job {job} did not reach the expected state");
    }
}

fn finished(s: &str) -> bool {
    matches!(s, "succeeded" | "failed" | "canceled")
}

#[sqlx::test(migrator = "wafer_api::MIGRATOR")]
async fn successful_run_with_log_and_artifacts(db: PgPool) {
    let h = harness(db, true).await;
    let ada = token("ada", "ada@example.org");
    let id = h.project(&ada, "chip").await;

    let (s, job) = h.call(Method::POST, &format!("/api/projects/{id}/jobs"), &ada, None).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{job}");
    assert_eq!(job["status"], "queued");
    assert_eq!(job["toolchain"], "fake@sha256:0");
    assert_eq!(job["pdk"], "ihp-sg13g2");
    assert_eq!(job["commit_id"].as_str().unwrap().len(), 40);
    let jid = job["id"].as_str().unwrap();

    let done = h.wait(&ada, jid, finished).await;
    assert_eq!(done["status"], "succeeded", "{done}");
    assert_eq!(done["summary"]["signoff_clean"], true);
    assert_eq!(done["summary"]["stdcells"], 56);

    let (_, log) = h.call(Method::GET, &format!("/api/jobs/{jid}/log"), &ada, None).await;
    assert!(log["content"].as_str().unwrap().contains("Flow complete."));
    assert_eq!(log["done"], true);
    let offset = log["offset"].as_u64().unwrap();
    let (_, rest) = h.call(Method::GET, &format!("/api/jobs/{jid}/log?offset={offset}"), &ada, None).await;
    assert_eq!(rest["content"], "");

    let (_, arts) = h.call(Method::GET, &format!("/api/jobs/{jid}/artifacts"), &ada, None).await;
    let paths: Vec<&str> = arts.as_array().unwrap().iter().map(|a| a["path"].as_str().unwrap()).collect();
    assert_eq!(paths, vec!["gds/chip.gds", "metrics.json"]);
    let (s, gds) = h.raw(Method::GET, &format!("/api/jobs/{jid}/artifacts/gds/chip.gds"), &ada, None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(gds, b"GDSII");
    let (s, _) = h.raw(Method::GET, &format!("/api/jobs/{jid}/artifacts/../flow.log"), &ada, None).await;
    assert_ne!(s, StatusCode::OK);

    let (_, list) = h.call(Method::GET, &format!("/api/projects/{id}/jobs"), &ada, None).await;
    assert_eq!(list.as_array().unwrap().len(), 1);
}

#[sqlx::test(migrator = "wafer_api::MIGRATOR")]
async fn failure_records_error_lines(db: PgPool) {
    let h = harness(db, true).await;
    let ada = token("ada", "ada@example.org");
    let id = h.project(&ada, "fail").await;
    let (_, job) = h.call(Method::POST, &format!("/api/projects/{id}/jobs"), &ada, None).await;
    let done = h.wait(&ada, job["id"].as_str().unwrap(), finished).await;
    assert_eq!(done["status"], "failed");
    assert_eq!(done["exit_code"], 1);
    assert!(done["error"].as_str().unwrap().contains("boom in placement"), "{done}");
}

#[sqlx::test(migrator = "wafer_api::MIGRATOR")]
async fn cancel_running_job(db: PgPool) {
    let h = harness(db, true).await;
    let ada = token("ada", "ada@example.org");
    let id = h.project(&ada, "slow").await;
    let (_, job) = h.call(Method::POST, &format!("/api/projects/{id}/jobs"), &ada, None).await;
    let jid = job["id"].as_str().unwrap();
    h.wait(&ada, jid, |s| s == "running").await;
    let (s, _) = h.call(Method::POST, &format!("/api/jobs/{jid}/cancel"), &ada, None).await;
    assert_eq!(s, StatusCode::OK);
    let done = h.wait(&ada, jid, finished).await;
    assert_eq!(done["status"], "canceled");
}

#[sqlx::test(migrator = "wafer_api::MIGRATOR")]
async fn validation_and_access(db: PgPool) {
    let h = harness(db, true).await;
    let ada = token("ada", "ada@example.org");
    let bob = token("bob", "bob@example.org");
    let id = h.project(&ada, "chip").await;

    // Config problems are caught before queuing.
    let (s, b) =
        h.call(Method::POST, &format!("/api/projects/{id}/jobs"), &ada, Some(json!({ "config": "nope.json" }))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{b}");
    h.call(
        Method::POST,
        &format!("/api/projects/{id}/commits"),
        &ada,
        Some(json!({ "message": "x", "changes": [{ "op": "write", "path": "bad.json", "content": "{}" }] })),
    )
    .await;
    let (s, b) =
        h.call(Method::POST, &format!("/api/projects/{id}/jobs"), &ada, Some(json!({ "config": "bad.json" }))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(b["message"].as_str().unwrap().contains("DESIGN_NAME"));
    let (s, _) =
        h.call(Method::POST, &format!("/api/projects/{id}/jobs"), &ada, Some(json!({ "commit": "deadbeef" }))).await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    // Outsiders can't see or start jobs; viewers can see but not start them.
    let (_, job) = h.call(Method::POST, &format!("/api/projects/{id}/jobs"), &ada, None).await;
    let jid = job["id"].as_str().unwrap();
    let (s, _) = h.call(Method::GET, &format!("/api/jobs/{jid}"), &bob, None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, _) = h.call(Method::GET, &format!("/api/jobs/{jid}/artifacts/metrics.json"), &bob, None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    h.call(
        Method::POST,
        &format!("/api/projects/{id}/members"),
        &ada,
        Some(json!({ "email": "bob@example.org", "role": "viewer" })),
    )
    .await;
    let (s, _) = h.call(Method::GET, &format!("/api/jobs/{jid}"), &bob, None).await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = h.call(Method::POST, &format!("/api/projects/{id}/jobs"), &bob, None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = h.call(Method::POST, &format!("/api/jobs/{jid}/cancel"), &bob, None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}

#[sqlx::test(migrator = "wafer_api::MIGRATOR")]
async fn no_backend_means_unavailable(db: PgPool) {
    let h = harness(db, false).await;
    let ada = token("ada", "ada@example.org");
    let id = h.project(&ada, "chip").await;
    let (s, _) = h.call(Method::POST, &format!("/api/projects/{id}/jobs"), &ada, None).await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE);
}
