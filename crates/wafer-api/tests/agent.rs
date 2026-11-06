//! The agent against a mock Claude Messages API: tool loop, gate, evidence, merge rules.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::routing::post;
use common::token;
use serde_json::{Value, json};
use sqlx::PgPool;
use wafer_api::agent::AgentService;
use wafer_api::claude::{Claude, ClaudeConfig};
use wafer_api::lsp::LspConfig;
use wafer_api::router;

type Seen = Arc<Mutex<Vec<(HeaderMap, Value)>>>;

const PARITY: &str = "module parity(input [7:0] d, output p);\n    assign p = ^d;\nendmodule\n";
const TB_PARITY: &str = "module tb_parity;\n    reg [7:0] d = 8'b1011_0001;\n    wire p;\n    parity u(.d(d), .p(p));\n    initial begin\n        #1 if (p == 1'b0) $display(\"PASS: parity\"); else $display(\"FAIL: parity %b\", p);\n        $finish;\n    end\nendmodule\n";
const TB_BAD: &str = "module tb_bad;\n    initial begin\n        $display(\"FAIL: expected 1, got 0\");\n        $finish;\n    end\nendmodule\n";

fn msg(stop: &str, content: Value) -> Value {
    json!({
        "id": "msg_test", "type": "message", "role": "assistant", "model": "claude-opus-5-5",
        "stop_reason": stop, "content": content,
        "usage": { "input_tokens": 100, "output_tokens": 50, "cache_read_input_tokens": 10, "cache_creation_input_tokens": 0 }
    })
}

fn tool(id: &str, name: &str, input: Value) -> Value {
    json!({ "type": "tool_use", "id": id, "name": name, "input": input })
}

/// Scripted Claude: the request text picks the scenario; the number of assistant turns so far picks the step.
async fn messages(State(seen): State<Seen>, headers: HeaderMap, Json(body): Json<Value>) -> Json<Value> {
    seen.lock().unwrap().push((headers, body.clone()));
    let msgs = body["messages"].as_array().unwrap();
    let request = msgs[0]["content"].as_str().unwrap_or_default().to_string();
    let turn = msgs.iter().filter(|m| m["role"] == "assistant").count();
    let thinking =
        json!({ "type": "thinking", "thinking": "Plan: add a parity module and a testbench.", "signature": "sig-abc" });
    Json(match (request.contains("parity"), request.contains("broken"), request.contains("refuse"), turn) {
        (_, _, true, _) => json!({
            "id": "msg_r", "type": "message", "role": "assistant", "model": "claude-opus-5-5", "stop_reason": "refusal",
            "stop_details": { "type": "refusal", "category": "cyber", "explanation": "test refusal" },
            "content": [], "usage": { "input_tokens": 5, "output_tokens": 0 }
        }),
        (true, _, _, 0) => msg(
            "tool_use",
            json!([thinking, { "type": "text", "text": "Looking at the project." },
            tool("t1", "list_files", json!({})), tool("t2", "read_file", json!({ "path": "config.json" }))]),
        ),
        (true, _, _, 1) => msg(
            "tool_use",
            json!([
                tool("t3", "write_file", json!({ "path": "src/parity.v", "content": PARITY })),
                tool("t4", "write_file", json!({ "path": "tb/tb_parity.v", "content": TB_PARITY })),
                tool("t5", "run_simulation", json!({ "top": "tb_parity" }))
            ]),
        ),
        (_, true, _, 0) => {
            msg("tool_use", json!([tool("b1", "write_file", json!({ "path": "tb/tb_bad.v", "content": TB_BAD }))]))
        }
        // The first parity request finishes with submit_proposal; the rest end with plain text.
        (true, _, _, 2) if !request.contains("again") => msg(
            "tool_use",
            json!([tool(
                "t6",
                "submit_proposal",
                json!({
                    "title": "Add 8-bit parity generator with testbench.",
                    "summary": "Added an 8-bit parity generator with a testbench; tb_parity passes."
                })
            )]),
        ),
        (_, _, _, _) => msg(
            "end_turn",
            json!([{ "type": "text", "text": "Added an 8-bit parity generator with a testbench; tb_parity passes." }]),
        ),
    })
}

struct Env {
    addr: String,
    seen: Seen,
    _dir: tempfile::TempDir,
}

async fn env(db: PgPool) -> Env {
    let seen: Seen = Arc::default();
    let mock = axum::Router::new().route("/v1/messages", post(messages)).with_state(seen.clone());
    let ml = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mock_url = format!("http://{}", ml.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(ml, mock).await.unwrap() });

    let (mut state, dir) = common::state(db, LspConfig::disabled());
    state.agent = Arc::new(AgentService::new(
        Some(Claude::new(ClaudeConfig {
            api_key: "test-key".into(),
            base_url: mock_url,
            model: "claude-opus-5-5".into(),
            effort: "high".into(),
            max_tokens: 16_000,
            bearer: false,
            fallbacks: true,
        })),
        2,
    ));
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    tokio::spawn(async move { axum::serve(l, router(state)).await.unwrap() });
    Env { addr, seen, _dir: dir }
}

async fn call(addr: &str, tok: &str, method: reqwest::Method, path: &str, body: Option<Value>) -> (u16, Value) {
    let mut req = reqwest::Client::new().request(method, format!("http://{addr}/api{path}")).bearer_auth(tok);
    if let Some(b) = body {
        req = req.json(&b);
    }
    let res = req.send().await.unwrap();
    let status = res.status().as_u16();
    (status, res.json().await.unwrap_or(Value::Null))
}

async fn wait_done(addr: &str, tok: &str, id: &str) -> Value {
    for _ in 0..200 {
        let (_, p) = call(addr, tok, reqwest::Method::GET, &format!("/agent/{id}"), None).await;
        if p["status"] != "running" {
            return p;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("proposal {id} did not finish");
}

async fn propose(e: &Env, tok: &str, project: &str, request: &str) -> Value {
    let (s, p) = call(
        &e.addr,
        tok,
        reqwest::Method::POST,
        &format!("/projects/{project}/agent"),
        Some(json!({ "request": request })),
    )
    .await;
    assert_eq!(s, 202, "{p}");
    wait_done(&e.addr, tok, p["id"].as_str().unwrap()).await
}

#[sqlx::test(migrator = "wafer_api::MIGRATOR")]
async fn proposal_gate_evidence_and_merge(db: PgPool) {
    let e = env(db).await;
    let ada = token("ada", "ada@example.org");
    let (_, p) = call(&e.addr, &ada, reqwest::Method::POST, "/projects", Some(json!({ "name": "logic" }))).await;
    let project = p["id"].as_str().unwrap().to_string();
    let base = p["head"].as_str().unwrap().to_string();

    let a = propose(&e, &ada, &project, "Add an 8-bit parity generator").await;
    assert_eq!(a["status"], "ready", "{a}");
    assert_eq!(a["base_commit"], base.as_str());
    assert_eq!(a["files"]["src/parity.v"], PARITY);
    assert!(a["base_files"]["src/parity.v"].is_null());
    // The gate ran every testbench: the template's and the agent's.
    let tops: Vec<&str> = a["gate"]["runs"].as_array().unwrap().iter().map(|r| r["top"].as_str().unwrap()).collect();
    assert_eq!(tops.len(), 2, "{tops:?}");
    assert!(tops.contains(&"tb_parity") && tops.contains(&"tb_top_logic"));
    assert_eq!(a["gate"]["passed"], true);

    // Evidence: steps, reasoning, models, usage, file hashes, and a hash over it all.
    let ev = &a["evidence"];
    let kinds: Vec<&str> = ev["steps"].as_array().unwrap().iter().map(|s| s["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, vec!["reasoning", "message", "tool", "tool", "tool", "tool", "tool", "tool"]);
    assert_eq!(a["title"], "Add 8-bit parity generator with testbench.");
    assert_eq!(a["summary"], "Added an 8-bit parity generator with a testbench; tb_parity passes.");
    assert_eq!(ev["title"], a["title"]);
    let sim = ev["steps"].as_array().unwrap().iter().find(|s| s["name"] == "run_simulation").unwrap();
    assert!(sim["output"].as_str().unwrap().contains("PASS: parity"), "{sim}");
    let write = ev["steps"].as_array().unwrap().iter().find(|s| s["name"] == "write_file").unwrap();
    assert!(write["input"]["sha256"].as_str().unwrap().len() == 64 && write["input"].get("content").is_none());
    assert_eq!(ev["models_served"], json!(["claude-opus-5-5"]));
    assert_eq!(ev["usage"]["output_tokens"], 150);
    assert_eq!(a["evidence_hash"].as_str().unwrap().len(), 64);

    // What we sent to Claude.
    {
        let seen = e.seen.lock().unwrap();
        let (h, body) = &seen[0];
        assert_eq!(h["x-api-key"], "test-key");
        assert_eq!(h["anthropic-version"], "2023-06-01");
        assert_eq!(h["anthropic-beta"], "server-side-fallback-2026-07-01");
        assert_eq!(body["model"], "claude-opus-5-5");
        assert_eq!(body["fallbacks"], "default");
        assert_eq!(body["thinking"]["type"], "adaptive");
        assert_eq!(body["output_config"]["effort"], "high");
        assert!(body.get("tool_choice").is_none(), "no forced tool use on Opus 5.5");
        assert!(body["tools"].as_array().unwrap().iter().all(|t| t["strict"] == true));
        // Turn 2 replays turn 1's assistant content unchanged (thinking + signature) and all results in one message.
        let msgs = seen[1].1["messages"].as_array().unwrap();
        assert_eq!(msgs[1]["content"][0]["signature"], "sig-abc");
        let results = msgs[2]["content"].as_array().unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r["type"] == "tool_result"));
    }

    // The gate reproduces on the recorded inputs.
    let (_, rg) =
        call(&e.addr, &ada, reqwest::Method::POST, &format!("/agent/{}/regate", a["id"].as_str().unwrap()), None).await;
    assert_eq!(rg["reproduced"], true, "{rg}");

    // Before and after the proposal: this project has no spec, so no examples or coverage, and
    // neither version has a state machine.
    let (s, cmp) =
        call(&e.addr, &ada, reqwest::Method::POST, &format!("/agent/{}/compare", a["id"].as_str().unwrap()), None)
            .await;
    assert_eq!(s, 200, "{cmp}");
    for side in ["before", "after"] {
        assert_eq!(cmp[side]["examples"], 0, "{cmp}");
        assert!(cmp[side]["coverage"].is_null() && cmp[side]["machines"] == json!([]), "{cmp}");
    }
    let bob = token("bob", "bob@example.org");
    let (s, _) =
        call(&e.addr, &bob, reqwest::Method::POST, &format!("/agent/{}/compare", a["id"].as_str().unwrap()), None)
            .await;
    assert_eq!(s, 404, "only members can compare");

    // A second proposal on the same base, to be made stale by merging the first.
    let b = propose(&e, &ada, &project, "Add a parity generator (again)").await;
    assert_eq!(b["status"], "ready");
    assert!(b["title"].is_null(), "a plain-text finish has no title");
    assert!(b["summary"].as_str().unwrap().contains("tb_parity passes"));

    let (s, m) =
        call(&e.addr, &ada, reqwest::Method::POST, &format!("/agent/{}/merge", a["id"].as_str().unwrap()), None).await;
    assert_eq!(s, 200, "{m}");
    assert_eq!(m["status"], "merged");
    let commit = m["merged_commit"].as_str().unwrap();
    let (_, log) = call(&e.addr, &ada, reqwest::Method::GET, &format!("/projects/{project}/commits"), None).await;
    assert_eq!(log[0]["id"], commit);
    let message = log[0]["message"].as_str().unwrap();
    assert!(message.starts_with("Add 8-bit parity generator with testbench\n\nAdded an 8-bit"), "{message}");
    assert!(message.contains(&format!("Evidence: sha256:{}", a["evidence_hash"].as_str().unwrap())), "{message}");
    assert!(message.contains("Simulation gate: passed (2 of 2 testbenches passed"), "{message}");
    let (_, f) =
        call(&e.addr, &ada, reqwest::Method::GET, &format!("/projects/{project}/files/src/parity.v"), None).await;
    assert_eq!(f["content"], PARITY);

    let (s, err) =
        call(&e.addr, &ada, reqwest::Method::POST, &format!("/agent/{}/merge", b["id"].as_str().unwrap()), None).await;
    assert_eq!(s, 409);
    assert!(err["message"].as_str().unwrap().contains("new commits"), "{err}");
    let (s, _) =
        call(&e.addr, &ada, reqwest::Method::POST, &format!("/agent/{}/merge", a["id"].as_str().unwrap()), None).await;
    assert_eq!(s, 409, "already merged");
}

#[sqlx::test(migrator = "wafer_api::MIGRATOR")]
async fn failing_gate_blocks_merge_and_refusals_fail(db: PgPool) {
    let e = env(db).await;
    let ada = token("ada", "ada@example.org");
    let bob = token("bob", "bob@example.org");
    let (_, p) = call(&e.addr, &ada, reqwest::Method::POST, "/projects", Some(json!({ "name": "logic" }))).await;
    let project = p["id"].as_str().unwrap().to_string();

    let bad = propose(&e, &ada, &project, "This will leave a broken testbench").await;
    assert_eq!(bad["status"], "gate_failed", "{bad}");
    assert!(bad["gate"]["summary"].as_str().unwrap().contains("tb_bad"));
    let (s, err) =
        call(&e.addr, &ada, reqwest::Method::POST, &format!("/agent/{}/merge", bad["id"].as_str().unwrap()), None)
            .await;
    assert_eq!(s, 409);
    assert!(err["message"].as_str().unwrap().contains("gate"), "{err}");
    let (s, d) =
        call(&e.addr, &ada, reqwest::Method::POST, &format!("/agent/{}/discard", bad["id"].as_str().unwrap()), None)
            .await;
    assert_eq!((s, d["status"].as_str()), (200, Some("discarded")));

    let refused = propose(&e, &ada, &project, "Please refuse this").await;
    assert_eq!(refused["status"], "failed");
    assert!(refused["error"].as_str().unwrap().contains("test refusal"), "{refused}");

    // Viewers can read proposals but not start or merge them; outsiders see nothing.
    let (s, _) =
        call(&e.addr, &bob, reqwest::Method::GET, &format!("/agent/{}", bad["id"].as_str().unwrap()), None).await;
    assert_eq!(s, 404);
    call(&e.addr, &bob, reqwest::Method::GET, "/me", None).await;
    call(
        &e.addr,
        &ada,
        reqwest::Method::POST,
        &format!("/projects/{project}/members"),
        Some(json!({ "email": "bob@example.org", "role": "viewer" })),
    )
    .await;
    let (s, _) = call(&e.addr, &bob, reqwest::Method::GET, &format!("/projects/{project}/agent"), None).await;
    assert_eq!(s, 200);
    let (s, _) = call(
        &e.addr,
        &bob,
        reqwest::Method::POST,
        &format!("/projects/{project}/agent"),
        Some(json!({ "request": "x" })),
    )
    .await;
    assert_eq!(s, 403);
}

#[sqlx::test(migrator = "wafer_api::MIGRATOR")]
async fn without_a_key_the_agent_is_unavailable(db: PgPool) {
    let (state, _dir) = common::state(db, LspConfig::disabled());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    tokio::spawn(async move { axum::serve(l, router(state)).await.unwrap() });
    let ada = token("ada", "ada@example.org");
    let (_, p) = call(&addr, &ada, reqwest::Method::POST, "/projects", Some(json!({ "name": "x" }))).await;
    let (s, _) = call(
        &addr,
        &ada,
        reqwest::Method::POST,
        &format!("/projects/{}/agent", p["id"].as_str().unwrap()),
        Some(json!({ "request": "x" })),
    )
    .await;
    assert_eq!(s, 503);
}
