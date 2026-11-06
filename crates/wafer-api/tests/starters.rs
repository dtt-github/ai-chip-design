//! Starter designs: listing them, starting a project from one (its examples pass from the first
//! commit), and adding one to an existing project so the Blocks view can place it.

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

fn bob_token() -> String {
    token("bob", "bob@example.org")
}

async fn call(app: &Router, method: Method, uri: &str, tok: &str, body: Option<Value>) -> (StatusCode, Value) {
    let req = Request::builder().method(method).uri(uri).header("authorization", format!("Bearer {tok}"));
    let req = match body {
        Some(b) => req.header("content-type", "application/json").body(Body::from(b.to_string())),
        None => req.body(Body::empty()),
    }
    .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (status, if bytes.is_empty() { Value::Null } else { serde_json::from_slice(&bytes).unwrap() })
}

#[sqlx::test(migrator = "wafer_api::MIGRATOR")]
async fn start_from_and_add_starters(db: PgPool) {
    let (state, _dir) = common::state(db, LspConfig::disabled());
    let app = router(state);
    let ada = token("ada", "ada@example.org");

    // The gallery.
    let (s, list) = call(&app, Method::GET, "/api/starters", &ada, None).await;
    assert_eq!(s, StatusCode::OK);
    let list = list.as_array().unwrap();
    assert_eq!(list.len(), 16);
    let tx = list.iter().find(|x| x["id"] == "uart_tx").expect("uart_tx");
    assert_eq!(tx["title"], "UART transmitter");
    assert_eq!(tx["category"], "Communication");
    assert_eq!(tx["top"], "uart_tx");
    assert_eq!(tx["clock_mhz"], 50.0);
    assert_eq!((tx["clock"].as_str(), tx["reset"].as_str()), (Some("clk"), Some("rst_n")));
    assert!(tx["ports"].as_array().unwrap().iter().any(|p| p["name"] == "busy" && p["dir"] == "out"));
    assert_eq!(tx["examples"], 3);
    assert!(tx.get("rtl").is_none() && tx.get("spec").is_none(), "the source stays server-side");

    // A project started from a design: its examples pass straight away.
    let (s, p) =
        call(&app, Method::POST, "/api/projects", &ada, Some(json!({ "name": "My UART", "starter": "uart_tx" }))).await;
    assert_eq!(s, StatusCode::CREATED, "{p}");
    let id = p["id"].as_str().unwrap().to_string();
    let (_, run) = call(&app, Method::POST, &format!("/api/projects/{id}/spec/run"), &ada, None).await;
    let verdicts: Vec<&str> =
        run["examples"].as_array().unwrap().iter().map(|e| e["verdict"].as_str().unwrap()).collect();
    assert_eq!(verdicts, vec!["pass", "pass", "pass"], "{run}");
    let (_, cfg) = call(&app, Method::GET, &format!("/api/projects/{id}/files/config.json"), &ada, None).await;
    let cfg: Value = serde_json::from_str(cfg["content"].as_str().unwrap()).unwrap();
    assert_eq!(cfg["DESIGN_NAME"], "uart_tx");
    assert_eq!(cfg["CLOCK_PERIOD"], 20.0);
    let (s, e) =
        call(&app, Method::POST, "/api/projects", &ada, Some(json!({ "name": "Nope", "starter": "warp_drive" }))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(e["message"].as_str().unwrap().contains("no starter design 'warp_drive'"), "{e}");

    // Adding starters to it: they become blocks, listed in the flow config.
    let (s, a) = call(&app, Method::POST, &format!("/api/projects/{id}/starters/fifo4"), &ada, None).await;
    assert_eq!(s, StatusCode::OK, "{a}");
    assert_eq!(a["path"], "starters/fifo4.v");
    let (_, cfg) = call(&app, Method::GET, &format!("/api/projects/{id}/files/config.json"), &ada, None).await;
    let cfg: Value = serde_json::from_str(cfg["content"].as_str().unwrap()).unwrap();
    assert_eq!(cfg["VERILOG_FILES"], json!(["dir::src/uart_tx.v", "dir::starters/fifo4.v"]));
    let (_, blocks) = call(&app, Method::GET, &format!("/api/projects/{id}/blocks"), &ada, None).await;
    let fifo =
        blocks["modules"].as_array().unwrap().iter().find(|m| m["name"] == "fifo4").expect("fifo4 in the palette");
    assert_eq!(fifo["source"], "starter");
    assert!(fifo["description"].as_str().unwrap().contains("4-byte queue"));

    // Twice, or over a module of the same name, is refused.
    let (s, e) = call(&app, Method::POST, &format!("/api/projects/{id}/starters/fifo4"), &ada, None).await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert!(e["message"].as_str().unwrap().contains("already in the project"), "{e}");
    let (s, e) = call(&app, Method::POST, &format!("/api/projects/{id}/starters/uart_tx"), &ada, None).await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert!(e["message"].as_str().unwrap().contains("already has a module called uart_tx"), "{e}");
    let (s, _) = call(&app, Method::POST, &format!("/api/projects/{id}/starters/nothing"), &ada, None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    // Coverage: the starter's examples try every decision; drop one and the gap is named.
    assert_eq!(run["coverage"]["covered"], run["coverage"]["total"], "{run}");
    let (_, b) =
        call(&app, Method::POST, "/api/projects", &ada, Some(json!({ "name": "Blink", "starter": "blinker" }))).await;
    let bid = b["id"].as_str().unwrap().to_string();
    let (_, v) = call(&app, Method::GET, &format!("/api/projects/{bid}/spec"), &ada, None).await;
    let mut spec = v["spec"].clone();
    spec["examples"] = json!([spec["examples"][0].clone()]);
    let (s, _) = call(
        &app,
        Method::PUT,
        &format!("/api/projects/{bid}/spec"),
        &ada,
        Some(json!({ "spec": spec, "parent": v["head"] })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (_, run) = call(&app, Method::POST, &format!("/api/projects/{bid}/spec/run"), &ada, None).await;
    let cov = &run["coverage"];
    assert!(cov["covered"].as_u64() < cov["total"].as_u64(), "{cov}");
    let gaps: Vec<&str> = cov["gaps"].as_array().unwrap().iter().map(|g| g["text"].as_str().unwrap()).collect();
    assert!(gaps.contains(&"Never tried: `en` = 0"), "{gaps:?}");
    assert!(cov["gaps"][0]["suggestion"].as_str().unwrap().contains("src/blinker.v"), "{cov}");

    // The playground's testbench: one example compiled without saving, exactly as Save would.
    let try_it = json!({ "id": "try_it", "title": "Tried in the playground", "params": [], "steps": [
        { "op": "set", "signal": "en", "value": "1" },
        { "op": "wait", "cycles": 4 },
        { "op": "expect", "signal": "led", "value": "1" }
    ]});
    let (s, tb) = call(
        &app,
        Method::POST,
        &format!("/api/projects/{bid}/spec/testbench"),
        &ada,
        Some(json!({ "spec": spec, "example": try_it })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{tb}");
    assert_eq!(tb["path"], "tb/examples/tb_ex_try_it.v");
    assert_eq!(tb["top"], "tb_ex_try_it");
    assert!(
        tb["source"].as_str().unwrap().contains("blinker #(.N(4))")
            || tb["source"].as_str().unwrap().contains("blinker dut")
    );
    let (_, files) = call(&app, Method::GET, &format!("/api/projects/{bid}/tree"), &ada, None).await;
    assert!(!files.to_string().contains("tb_ex_try_it"), "nothing is saved");
    let mut bad = try_it.clone();
    bad["steps"][0]["signal"] = json!("nope");
    let (s, e) = call(
        &app,
        Method::POST,
        &format!("/api/projects/{bid}/spec/testbench"),
        &ada,
        Some(json!({ "spec": spec, "example": bad })),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(e["message"].as_str().unwrap().contains("'nope' is not one of the connections"), "{e}");

    // The receiver's state machine, every transition taken by its examples except ones they skip.
    let (_, r) =
        call(&app, Method::POST, "/api/projects", &ada, Some(json!({ "name": "Rx", "starter": "uart_rx" }))).await;
    let rid = r["id"].as_str().unwrap().to_string();
    let (_, run) = call(&app, Method::POST, &format!("/api/projects/{rid}/spec/run"), &ada, None).await;
    let m = &run["machines"][0];
    assert_eq!(m["state"], "state", "{run}");
    assert_eq!(m["states"], json!(["0", "1", "2", "3"]));
    let ts = m["transitions"].as_array().unwrap();
    assert_eq!(ts.len(), 5, "{m}");
    // Both sides of `state <= rx ? 0 : 2` are in one branch: whether each was taken can't be told.
    let glitch = ts.iter().find(|t| t["from"] == "1" && t["to"] == "0").unwrap();
    assert!(glitch["covered"].is_null(), "{glitch}");
    let taken =
        |from: &str, to: &str| ts.iter().find(|t| t["from"] == from && t["to"] == to).unwrap()["covered"].clone();
    assert_eq!((taken("0", "1"), taken("2", "3"), taken("3", "0")), (json!(true), json!(true), json!(true)));

    // Planted mistakes: the receiver's examples miss a few; the blinker with one example misses none.
    let (s, m) = call(&app, Method::POST, &format!("/api/projects/{rid}/spec/mistakes"), &ada, None).await;
    assert_eq!(s, StatusCode::OK, "{m}");
    assert!(m["caught"].as_u64() < m["tried"].as_u64() && m["tried"].as_u64() == m["sites"].as_u64(), "{m}");
    assert!(m["slipped"][0]["text"].as_str().unwrap().ends_with("every example would still pass."), "{m}");
    let (s, _) = call(&app, Method::POST, &format!("/api/projects/{rid}/spec/mistakes"), &bob_token(), None).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "only members can run it");

    // The gallery shows each starter's figure.
    let (_, list) = call(&app, Method::GET, "/api/starters", &ada, None).await;
    let fig = |id: &str| list.as_array().unwrap().iter().find(|x| x["id"] == id).unwrap()["covered"].clone();
    assert_eq!(fig("uart_tx"), json!([10, 10]));
    assert_eq!(fig("traffic"), json!([11, 12]));
    assert!(fig("parity8").is_null());

    // Only editors can add.
    let bob = token("bob", "bob@example.org");
    let (s, _) = call(&app, Method::POST, &format!("/api/projects/{id}/starters/blinker"), &bob, None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}
