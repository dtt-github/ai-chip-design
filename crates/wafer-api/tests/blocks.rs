//! The Blocks view's API: modules to place, checks, saving the chip (generated top, adapters,
//! flow config) in one commit.

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

const SINK: &str = "// SPDX-License-Identifier: Apache-2.0
module sink (input wire clk, input wire rst_n, input wire start, input wire [7:0] data, output reg got);
  always @(posedge clk or negedge rst_n) if (!rst_n) got <= 0; else got <= start;
endmodule
";

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

fn chip(top: &str, data_from: &str) -> Value {
    json!({
        "top": top, "clock": "clk", "reset": "rst_n",
        "pins": [
            { "name": "clk", "dir": "in", "width": 1 },
            { "name": "rst_n", "dir": "in", "width": 1 },
            { "name": "go", "dir": "in", "width": 1, "meaning": "Button" },
            { "name": "got", "dir": "out", "width": 1 }
        ],
        "blocks": [
            { "name": "cnt", "module": "pulse", "x": 40, "y": 40 },
            { "name": "edge", "module": "wafer_rise_pulse", "x": 40, "y": 200 },
            { "name": "snk", "module": "sink", "x": 300, "y": 100 }
        ],
        "wires": [
            { "from": "go", "to": "cnt.en" },
            { "from": "go", "to": "edge.level" },
            { "from": "edge.pulse", "to": "snk.start" },
            { "from": data_from, "to": "snk.data" },
            { "from": "snk.got", "to": "got" }
        ]
    })
}

#[sqlx::test(migrator = "wafer_api::MIGRATOR")]
async fn blocks_are_checked_saved_and_built(db: PgPool) {
    let (state, _dir) = common::state(db, LspConfig::disabled());
    let app = router(state);
    let ada = token("ada", "ada@example.org");
    let (_, p) = call(&app, Method::POST, "/api/projects", &ada, Some(json!({ "name": "Pulse" }))).await;
    let id = p["id"].as_str().unwrap().to_string();
    let head = p["head"].as_str().unwrap().to_string();
    let body = json!({ "message": "add a sink", "parent": head, "changes": [{ "op": "write", "path": "src/sink.v", "content": SINK }] });
    let (_, c) = call(&app, Method::POST, &format!("/api/projects/{id}/commits"), &ada, Some(body)).await;
    let head = c["id"].as_str().unwrap().to_string();

    // Modules to place: the project's, then the adapters; testbenches are not blocks.
    let (s, v) = call(&app, Method::GET, &format!("/api/projects/{id}/blocks"), &ada, None).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert!(v["blocks"].is_null());
    assert_eq!(v["design_name"], "pulse");
    let names: Vec<&str> = v["modules"].as_array().unwrap().iter().map(|m| m["name"].as_str().unwrap()).collect();
    assert_eq!(&names[..2], ["pulse", "sink"], "{names:?}");
    assert!(names.contains(&"wafer_rise_pulse") && !names.iter().any(|n| n.starts_with("tb_")));

    // A width mismatch is explained and nothing is generated.
    let (_, ch) = call(
        &app,
        Method::POST,
        &format!("/api/projects/{id}/blocks/check"),
        &ada,
        Some(json!({ "blocks": chip("chip", "go") })),
    )
    .await;
    assert!(ch["verilog"].is_null());
    assert!(
        ch["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["message"].as_str().unwrap().contains("is 1 bit but snk.data is 8 bits")),
        "{ch}"
    );
    let (s, _) = call(
        &app,
        Method::PUT,
        &format!("/api/projects/{id}/blocks"),
        &ada,
        Some(json!({ "blocks": chip("chip", "go"), "parent": head })),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // A chip named like a hand-written module file can't overwrite it.
    let (s, e) = call(
        &app,
        Method::PUT,
        &format!("/api/projects/{id}/blocks"),
        &ada,
        Some(json!({ "blocks": chip("pulse", "cnt.count"), "parent": head })),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{e}");

    // A good chip: blocks.json, the top, the adapters and the flow config in one commit.
    let (s, r) = call(
        &app,
        Method::PUT,
        &format!("/api/projects/{id}/blocks"),
        &ada,
        Some(json!({ "blocks": chip("chip", "cnt.count"), "parent": head })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{r}");
    let file = |path: &str| {
        let app = app.clone();
        let (id, ada, path) = (id.clone(), ada.clone(), path.to_string());
        async move {
            let (s, f) = call(&app, Method::GET, &format!("/api/projects/{id}/files/{path}"), &ada, None).await;
            assert_eq!(s, StatusCode::OK, "{path}");
            f["content"].as_str().unwrap().to_string()
        }
    };
    let top = file("src/chip.v").await;
    assert!(top.starts_with("// Generated by Wafersmith from blocks.json"), "{top}");
    assert!(top.contains("module chip (") && top.contains(".data(cnt__count)") && top.contains(".clk(clk)"), "{top}");
    assert!(file("src/wafer_adapters.v").await.contains("module wafer_rise_pulse"));
    let cfg: Value = serde_json::from_str(&file("config.json").await).unwrap();
    assert_eq!(cfg["DESIGN_NAME"], "chip");
    assert_eq!(
        cfg["VERILOG_FILES"],
        json!(["dir::src/chip.v", "dir::src/pulse.v", "dir::src/sink.v", "dir::src/wafer_adapters.v"])
    );
    let (_, v) = call(&app, Method::GET, &format!("/api/projects/{id}/blocks"), &ada, None).await;
    assert_eq!(v["blocks"]["blocks"][2]["x"], 300.0);
    assert_eq!(v["check"]["issues"], json!([]), "{v}");
    assert!(!v["modules"].as_array().unwrap().iter().any(|m| m["name"] == "chip"), "the chip isn't a block of itself");

    // Without the adapter, the generated adapters file goes too.
    let head = r["commit"].as_str().unwrap().to_string();
    let mut plain = chip("chip", "cnt.count");
    plain["blocks"].as_array_mut().unwrap().remove(1);
    plain["wires"] = json!([{ "from": "go", "to": "cnt.en" }, { "from": "go", "to": "snk.start" }, { "from": "cnt.count", "to": "snk.data" }, { "from": "snk.got", "to": "got" }]);
    let (s, r) = call(
        &app,
        Method::PUT,
        &format!("/api/projects/{id}/blocks"),
        &ada,
        Some(json!({ "blocks": plain, "parent": head })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{r}");
    let (s, _) = call(&app, Method::GET, &format!("/api/projects/{id}/files/src/wafer_adapters.v"), &ada, None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    // Viewers can look but not save; the review needs the assistant.
    let bob = token("bob", "bob@example.org");
    let (s, _) = call(
        &app,
        Method::PUT,
        &format!("/api/projects/{id}/blocks"),
        &bob,
        Some(json!({ "blocks": chip("chip", "cnt.count"), "parent": head })),
    )
    .await;
    assert!(s == StatusCode::NOT_FOUND || s == StatusCode::FORBIDDEN);
    let (s, _) = call(
        &app,
        Method::POST,
        &format!("/api/projects/{id}/blocks/review"),
        &ada,
        Some(json!({ "blocks": chip("chip", "cnt.count") })),
    )
    .await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE);
}
