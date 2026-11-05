//! Intent-first design: the spec, examples compiled into testbenches, running
//! them, and the assistant drafting a spec (against a mock Messages API).

mod common;

use std::sync::{Arc, Mutex};

use axum::Json;
use axum::extract::State;
use axum::routing::post;
use common::token;
use sc_api::agent::AgentService;
use sc_api::claude::{Claude, ClaudeConfig};
use sc_api::lsp::LspConfig;
use sc_api::router;
use serde_json::{Value, json};
use sqlx::PgPool;

type Seen = Arc<Mutex<Vec<Value>>>;

fn counter_spec(expected: &str) -> Value {
    json!({
        "version": 1,
        "intent": "Count up while enabled.",
        "top": "pulse",
        "clock": { "port": "clk", "mhz": 50.0 },
        "reset": { "port": "rst_n", "active_low": true },
        "ports": [
            { "name": "clk", "dir": "in", "width": 1, "meaning": "Clock" },
            { "name": "rst_n", "dir": "in", "width": 1, "meaning": "Reset, 0 resets" },
            { "name": "en", "dir": "in", "width": 1, "meaning": "Count while on" },
            { "name": "count", "dir": "out", "width": 8, "meaning": "How many cycles en was on" }
        ],
        "params": [{ "name": "WIDTH", "value": "8", "meaning": "Counter size in bits" }],
        "examples": [{
            "id": "counts", "title": "Counts five cycles", "given": "en on for 5 cycles", "expect": "count is 5",
            "params": [],
            "steps": [
                { "op": "expect", "signal": "count", "value": "0" },
                { "op": "set", "signal": "en", "value": "1" },
                { "op": "wait", "cycles": 5 },
                { "op": "set", "signal": "en", "value": "0" },
                { "op": "expect", "signal": "count", "value": expected, "note": "five rising edges" }
            ]
        }]
    })
}

/// A mock model: the first reply submits a spec with a mistake, the second fixes it.
async fn messages(State(seen): State<Seen>, Json(body): Json<Value>) -> Json<Value> {
    seen.lock().unwrap().push(body.clone());
    let turn = body["messages"].as_array().unwrap().iter().filter(|m| m["role"] == "assistant").count();
    let mut spec = counter_spec("5");
    spec.as_object_mut().unwrap().remove("version");
    spec["decisions"] = json!([]);
    spec["questions"] = json!([{ "question": "Should count wrap at 255?", "options": ["Wrap to 0", "Stop at 255"] }]);
    for s in spec["examples"][0]["steps"].as_array_mut().unwrap() {
        for (k, v) in [("signal", json!("")), ("value", json!("")), ("cycles", json!(0)), ("note", json!(""))] {
            s.as_object_mut().unwrap().entry(k).or_insert(v);
        }
    }
    for e in spec["examples"].as_array_mut().unwrap() {
        e["assumes"] = json!([]);
    }
    let first = body["messages"][0]["content"].as_str().unwrap_or_default().to_string();
    if first.contains("assume something") && turn == 0 {
        // An example that depends on the open question: refused, then dropped on turn 1.
        spec["examples"][0]["assumes"] = json!(["Should count wrap at 255?"]);
    } else if first.contains("assume something") {
        spec["examples"] = json!([]);
    }
    if first.contains("decide for me") {
        // Turn 0: the model makes a choice itself; turn 1 (after the push back): it asks instead.
        if turn == 0 {
            spec["decisions"] = json!([{ "question": "Should count wrap at 255?", "answer": "Wrap to 0" }]);
            spec["questions"] = json!([]);
        }
    } else if turn == 0 {
        spec["examples"][0]["steps"][1]["signal"] = json!("enable");
    }
    Json(json!({
        "id": "msg", "type": "message", "role": "assistant", "model": "claude-opus-5-5", "stop_reason": "tool_use",
        "content": [{ "type": "tool_use", "id": format!("t{turn}"), "name": "submit_spec",
                      "input": { "spec": spec, "note": "Drafted a counter spec with one example." } }],
        "usage": { "input_tokens": 100, "output_tokens": 50 }
    }))
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

#[sqlx::test(migrator = "sc_api::MIGRATOR")]
async fn spec_examples_and_drafting(db: PgPool) {
    let seen: Seen = Arc::default();
    let seen_handle = seen.clone();
    let mock = axum::Router::new().route("/v1/messages", post(messages)).with_state(seen.clone());
    let ml = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mock_url = format!("http://{}", ml.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(ml, mock).await.unwrap() });
    let (mut state, _dir) = common::state(db, LspConfig::disabled());
    state.agent = Arc::new(AgentService::new(
        Some(Claude::new(ClaudeConfig {
            api_key: "k".into(),
            base_url: mock_url,
            model: "claude-opus-5-5".into(),
            effort: "high".into(),
            max_tokens: 16_000,
            bearer: false,
            fallbacks: false,
        })),
        1,
    ));
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    tokio::spawn(async move { axum::serve(l, router(state)).await.unwrap() });
    let ada = token("ada", "ada@example.org");
    use reqwest::Method as M;

    let (_, p) = call(&addr, &ada, M::POST, "/projects", Some(json!({ "name": "Pulse" }))).await;
    let id = p["id"].as_str().unwrap().to_string();
    let (s, v) = call(&addr, &ada, M::GET, &format!("/projects/{id}/spec"), None).await;
    assert_eq!(s, 200, "{v}");
    assert!(v["spec"].is_null());
    assert_eq!(v["design_name"], "pulse");
    assert_eq!(v["clock_mhz"], 50.0);
    let head = v["head"].as_str().unwrap().to_string();

    // Saving compiles the example into a testbench in the same commit; the template counter passes it.
    let (s, r) = call(
        &addr,
        &ada,
        M::PUT,
        &format!("/projects/{id}/spec"),
        Some(json!({ "spec": counter_spec("5"), "parent": head })),
    )
    .await;
    assert_eq!(s, 200, "{r}");
    let (_, f) = call(&addr, &ada, M::GET, &format!("/projects/{id}/files/tb/examples/tb_ex_counts.v"), None).await;
    assert!(f["content"].as_str().unwrap().contains("module tb_ex_counts"), "{f}");
    let (_, v) = call(&addr, &ada, M::GET, &format!("/projects/{id}/spec"), None).await;
    assert_eq!(v["in_sync"], true);
    assert_eq!(v["spec"]["examples"][0]["steps"][4]["note"], "five rising edges");
    let (s, run) = call(&addr, &ada, M::POST, &format!("/projects/{id}/spec/run"), None).await;
    assert_eq!(s, 200, "{run}");
    assert_eq!(run["examples"][0]["verdict"], "pass", "{run}");
    assert_eq!(run["others"][0]["top"], "tb_pulse");

    // A wrong expectation fails, saying which step and what it saw.
    let head = v["head"].as_str().unwrap().to_string();
    let (s, r) = call(
        &addr,
        &ada,
        M::PUT,
        &format!("/projects/{id}/spec"),
        Some(json!({ "spec": counter_spec("6"), "parent": head })),
    )
    .await;
    assert_eq!(s, 200, "{r}");
    let (_, run) = call(&addr, &ada, M::POST, &format!("/projects/{id}/spec/run"), None).await;
    assert_eq!(run["examples"][0]["verdict"], "fail");
    assert!(
        run["examples"][0]["log"][0].as_str().unwrap().contains("FAIL: step 5: expected count = 6, got 0x5"),
        "{run}"
    );

    // Stale parent, invalid spec, and a hand-written file in the way are refused.
    let (s, _) = call(
        &addr,
        &ada,
        M::PUT,
        &format!("/projects/{id}/spec"),
        Some(json!({ "spec": counter_spec("7"), "parent": head })),
    )
    .await;
    assert_eq!(s, 409);
    let head = r["commit"].as_str().unwrap().to_string();
    let mut bad = counter_spec("5");
    bad["examples"][0]["steps"][1]["signal"] = json!("count");
    let (s, e) =
        call(&addr, &ada, M::PUT, &format!("/projects/{id}/spec"), Some(json!({ "spec": bad, "parent": head }))).await;
    assert_eq!(s, 400);
    assert!(e["message"].as_str().unwrap().contains("can only set inputs"), "{e}");
    let body = json!({ "message": "hand-written", "parent": head, "changes": [{ "op": "write", "path": "tb/examples/tb_ex_mine.v", "content": "module tb_ex_mine; endmodule\n" }] });
    let (_, c) = call(&addr, &ada, M::POST, &format!("/projects/{id}/commits"), Some(body)).await;
    let head = c["id"].as_str().unwrap().to_string();
    let mut clash = counter_spec("5");
    clash["examples"][0]["id"] = json!("mine");
    let (s, _) =
        call(&addr, &ada, M::PUT, &format!("/projects/{id}/spec"), Some(json!({ "spec": clash, "parent": head })))
            .await;
    assert_eq!(s, 409);

    // Removing an example removes its generated testbench (and only generated ones).
    let mut none = counter_spec("5");
    none["examples"] = json!([]);
    let (s, _) =
        call(&addr, &ada, M::PUT, &format!("/projects/{id}/spec"), Some(json!({ "spec": none, "parent": head }))).await;
    assert_eq!(s, 200);
    let (s, _) = call(&addr, &ada, M::GET, &format!("/projects/{id}/files/tb/examples/tb_ex_counts.v"), None).await;
    assert_eq!(s, 404);
    let (s, _) = call(&addr, &ada, M::GET, &format!("/projects/{id}/files/tb/examples/tb_ex_mine.v"), None).await;
    assert_eq!(s, 200);

    // Drafting: the server checks the model's spec and sends problems back until it is valid.
    let (s, d) = call(
        &addr,
        &ada,
        M::POST,
        &format!("/projects/{id}/spec/draft"),
        Some(json!({ "instruction": "A counter that counts while enabled" })),
    )
    .await;
    assert_eq!(s, 200, "{d}");
    assert_eq!(d["spec"]["examples"][0]["steps"][1]["signal"], "en");
    assert_eq!(d["spec"]["questions"][0]["options"][1], "Stop at 255");
    assert_eq!(d["usage"]["output_tokens"], 100);
    // Two turns of 100 input and 50 output tokens, at the served model's prices (Opus 5.5: $4 / $20).
    assert!((d["usd"].as_f64().unwrap() - 0.0028).abs() < 1e-9, "{d}");
    {
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2);
        let feedback = &seen[1]["messages"].as_array().unwrap().last().unwrap()["content"][0];
        assert_eq!(feedback["is_error"], true);
        assert!(feedback["content"].as_str().unwrap().contains("'enable' is not one of the connections"), "{feedback}");
        assert!(
            seen[0]["messages"][0]["content"].as_str().unwrap().contains("--- src/pulse.v ---"),
            "the RTL is context"
        );
        assert_eq!(seen[0]["tools"][0]["strict"], true);
    }

    // Decisions the user didn't make are pushed back once, and become questions.
    let (s, d) = call(
        &addr,
        &ada,
        M::POST,
        &format!("/projects/{id}/spec/draft"),
        Some(json!({ "instruction": "A counter; decide for me" })),
    )
    .await;
    assert_eq!(s, 200, "{d}");
    assert_eq!(d["spec"]["decisions"], json!([]));
    assert_eq!(d["spec"]["questions"][0]["question"], "Should count wrap at 255?");
    {
        let seen = seen_handle.lock().unwrap();
        let feedback = &seen[3]["messages"].as_array().unwrap().last().unwrap()["content"][0];
        assert!(
            feedback["content"].as_str().unwrap().contains("These decisions are new: \"Should count wrap at 255?\""),
            "{feedback}"
        );
    }

    // Examples that depend on an open question are refused until rewritten or left out.
    let (s, d) = call(
        &addr,
        &ada,
        M::POST,
        &format!("/projects/{id}/spec/draft"),
        Some(json!({ "instruction": "A counter; assume something" })),
    )
    .await;
    assert_eq!(s, 200, "{d}");
    assert_eq!(d["spec"]["examples"], json!([]));
    let seen = seen_handle.lock().unwrap();
    let feedback = &seen[5]["messages"].as_array().unwrap().last().unwrap()["content"][0];
    assert!(
        feedback["content"].as_str().unwrap().contains("depends on the open question \"Should count wrap at 255?\""),
        "{feedback}"
    );
    // A spec without examples is pushed back once, asking for examples that hold whatever the answers.
    let feedback = &seen[6]["messages"].as_array().unwrap().last().unwrap()["content"][0];
    assert!(feedback["content"].as_str().unwrap().contains("There are no examples"), "{feedback}");
    assert_eq!(seen.len(), 7);
}

#[sqlx::test(migrator = "sc_api::MIGRATOR")]
async fn daily_ai_budget(db: PgPool) {
    let seen: Seen = Arc::default();
    let mock = axum::Router::new().route("/v1/messages", post(messages)).with_state(seen);
    let ml = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mock_url = format!("http://{}", ml.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(ml, mock).await.unwrap() });
    let (mut state, _dir) = common::state(db, LspConfig::disabled());
    let claude = Claude::new(ClaudeConfig {
        api_key: "k".into(),
        base_url: mock_url,
        model: "claude-opus-5-5".into(),
        effort: "high".into(),
        max_tokens: 16_000,
        bearer: false,
        fallbacks: false,
    });
    // Two mock turns cost 200 input + 100 output tokens = $0.0028; the limit is less.
    state.agent = Arc::new(AgentService::new(Some(claude), 1).with_daily_limit(Some(0.002)));
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    tokio::spawn(async move { axum::serve(l, router(state)).await.unwrap() });
    let ada = token("ada", "ada@example.org");
    let bob = token("bob", "bob@example.org");
    use reqwest::Method as M;

    let (_, p) = call(&addr, &ada, M::POST, "/projects", Some(json!({ "name": "Pulse" }))).await;
    let id = p["id"].as_str().unwrap().to_string();
    let draft = json!({ "instruction": "A counter that counts while enabled" });
    let (s, _) = call(&addr, &ada, M::POST, &format!("/projects/{id}/spec/draft"), Some(draft.clone())).await;
    assert_eq!(s, 200);
    let (_, u) = call(&addr, &ada, M::GET, "/me/ai-usage", None).await;
    assert!((u["today_usd"].as_f64().unwrap() - 0.0028).abs() < 1e-9, "{u}");
    assert_eq!(u["limit_usd"], 0.002);
    let (s, e) = call(&addr, &ada, M::POST, &format!("/projects/{id}/spec/draft"), Some(draft)).await;
    assert_eq!(s, 429, "{e}");
    assert!(e["message"].as_str().unwrap().contains("today's AI budget"), "{e}");
    let (s, _) =
        call(&addr, &ada, M::POST, &format!("/projects/{id}/agent"), Some(json!({ "request": "anything" }))).await;
    assert_eq!(s, 429, "the agent shares the budget");
    // Budgets are per person.
    let (_, u) = call(&addr, &bob, M::GET, "/me/ai-usage", None).await;
    assert_eq!(u["today_usd"], 0.0);
}
