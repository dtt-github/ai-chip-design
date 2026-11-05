//! Language-server bridge tests. They run the real `verible-verilog-ls`
//! (from `scripts/fetch-verible.sh` or `SC_VERIBLE_LS`) and are skipped,
//! with a note, when it is not installed.

mod common;

use std::path::PathBuf;
use std::time::Duration;

use common::token;
use futures_util::{SinkExt, StreamExt};
use sc_api::lsp::LspConfig;
use sc_api::router;
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio_tungstenite::tungstenite::Message;

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

fn verible() -> Option<PathBuf> {
    // Relative paths (as in .env) are relative to the repository root, not this crate.
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let path = std::env::var("SC_VERIBLE_LS").unwrap_or_else(|_| ".tools/verible/bin/verible-verilog-ls".into());
    root.join(path).canonicalize().ok()
}

/// Serves the app on a random port; returns its address.
async fn serve(db: PgPool, lsp: LspConfig) -> (String, tempfile::TempDir) {
    let (state, dir) = common::state(db, lsp);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    tokio::spawn(async move { axum::serve(listener, router(state)).await.unwrap() });
    (addr, dir)
}

async fn post(addr: &str, tok: &str, path: &str, body: Value) -> Value {
    let res = reqwest::Client::new()
        .post(format!("http://{addr}/api{path}"))
        .bearer_auth(tok)
        .json(&body)
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "{path}: {}", res.status());
    res.json().await.unwrap()
}

async fn connect(addr: &str, project: &str, tok: &str) -> (Ws, Value) {
    let (mut ws, _) =
        tokio_tungstenite::connect_async(format!("ws://{addr}/api/projects/{project}/lsp")).await.unwrap();
    ws.send(Message::text(json!({ "token": tok }).to_string())).await.unwrap();
    let hello = next_json(&mut ws).await.expect("hello reply");
    (ws, hello)
}

async fn next_json(ws: &mut Ws) -> Option<Value> {
    loop {
        match tokio::time::timeout(Duration::from_secs(20), ws.next()).await.expect("timed out waiting for message")? {
            Ok(Message::Text(t)) => return Some(serde_json::from_str(t.as_str()).unwrap()),
            Ok(Message::Close(_)) | Err(_) => return None,
            Ok(_) => {}
        }
    }
}

async fn send(ws: &mut Ws, msg: Value) {
    ws.send(Message::text(msg.to_string())).await.unwrap();
}

/// Waits for a message matching `pred`, answering server-to-client requests with `null`.
async fn wait_for(ws: &mut Ws, pred: impl Fn(&Value) -> bool) -> Value {
    loop {
        let msg = next_json(ws).await.expect("connection closed");
        if pred(&msg) {
            return msg;
        }
        if msg.get("method").is_some() && msg.get("id").is_some() {
            send(ws, json!({ "jsonrpc": "2.0", "id": msg["id"], "result": null })).await;
        }
    }
}

#[sqlx::test(migrator = "sc_api::MIGRATOR")]
async fn refuses_bad_token_and_disabled_server(db: PgPool) {
    let (addr, _dir) = serve(db, LspConfig::disabled()).await;
    let ada = token("ada", "ada@example.org");
    let p = post(&addr, &ada, "/projects", json!({ "name": "x" })).await;
    let id = p["id"].as_str().unwrap();

    let (_, hello) = connect(&addr, id, "not-a-token").await;
    assert_eq!(hello["error"], "unauthorized");
    let (_, hello) = connect(&addr, id, &token("eve", "eve@example.org")).await;
    assert_eq!(hello["error"], "not_found");
    let (_, hello) = connect(&addr, id, &ada).await;
    assert_eq!(hello["error"], "unavailable");
}

#[sqlx::test(migrator = "sc_api::MIGRATOR")]
async fn diagnostics_and_cross_file_definition(db: PgPool) {
    let Some(bin) = verible() else {
        eprintln!("skipping: verible-verilog-ls not installed (run scripts/fetch-verible.sh)");
        return;
    };
    let (addr, _dir) = serve(db, LspConfig::new(Some(bin), 4)).await;
    let ada = token("ada", "ada@example.org");
    let p = post(&addr, &ada, "/projects", json!({ "name": "My Counter" })).await;
    let id = p["id"].as_str().unwrap().to_string();
    let top =
        "module top (input wire clk);\n  my_counter u0 (.clk(clk), .rst_n(1'b1), .en(1'b1), .count());\nendmodule\n";
    post(
        &addr,
        &ada,
        &format!("/projects/{id}/commits"),
        json!({ "message": "top", "changes": [{ "op": "write", "path": "src/top.v", "content": top }] }),
    )
    .await;

    let (mut ws, hello) = connect(&addr, &id, &ada).await;
    assert_eq!(hello["type"], "ready", "{hello}");

    send(
        &mut ws,
        json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": { "processId": null, "rootUri": null, "capabilities": {} } }),
    )
    .await;
    let init = wait_for(&mut ws, |m| m["id"] == 1).await;
    assert!(init["result"]["capabilities"]["definitionProvider"].as_bool().unwrap_or(false), "{init}");
    send(&mut ws, json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} })).await;

    // A syntax error is reported against the client's URI.
    let bad_uri = format!("file:///{id}/src/bad.v");
    send(
        &mut ws,
        json!({ "jsonrpc": "2.0", "method": "textDocument/didOpen", "params": { "textDocument": {
        "uri": bad_uri, "languageId": "verilog", "version": 1, "text": "module bad(;\nendmodule\n" } } }),
    )
    .await;
    let diag = wait_for(&mut ws, |m| {
        m["method"] == "textDocument/publishDiagnostics" && m["params"]["uri"] == bad_uri.as_str()
    })
    .await;
    assert!(!diag["params"]["diagnostics"].as_array().unwrap().is_empty(), "{diag}");

    // Go to definition of `my_counter` lands in the other project file.
    let top_uri = format!("file:///{id}/src/top.v");
    send(
        &mut ws,
        json!({ "jsonrpc": "2.0", "method": "textDocument/didOpen", "params": { "textDocument": {
        "uri": top_uri, "languageId": "verilog", "version": 1, "text": top } } }),
    )
    .await;
    send(
        &mut ws,
        json!({ "jsonrpc": "2.0", "id": 2, "method": "textDocument/definition",
        "params": { "textDocument": { "uri": top_uri }, "position": { "line": 1, "character": 5 } } }),
    )
    .await;
    let def = wait_for(&mut ws, |m| m["id"] == 2).await;
    let locations = def["result"].as_array().cloned().unwrap_or_else(|| vec![def["result"].clone()]);
    assert_eq!(locations[0]["uri"], format!("file:///{id}/src/my_counter.v"), "{def}");

    // Formatting a badly indented file returns edits.
    let ugly_uri = format!("file:///{id}/src/ugly.v");
    send(
        &mut ws,
        json!({ "jsonrpc": "2.0", "method": "textDocument/didOpen", "params": { "textDocument": {
        "uri": ugly_uri, "languageId": "verilog", "version": 1,
        "text": "module ugly(input a, output b);\n            assign b=a;\nendmodule\n" } } }),
    )
    .await;
    send(
        &mut ws,
        json!({ "jsonrpc": "2.0", "id": 5, "method": "textDocument/formatting",
        "params": { "textDocument": { "uri": ugly_uri }, "options": { "tabSize": 2, "insertSpaces": true } } }),
    )
    .await;
    let fmt = wait_for(&mut ws, |m| m["id"] == 5).await;
    assert!(!fmt["result"].as_array().map(Vec::is_empty).unwrap_or(true), "{fmt}");

    // A message naming a file outside the project never reaches the server.
    send(
        &mut ws,
        json!({ "jsonrpc": "2.0", "id": 3, "method": "textDocument/definition",
        "params": { "textDocument": { "uri": "file:///etc/passwd" }, "position": { "line": 0, "character": 0 } } }),
    )
    .await;
    send(&mut ws, json!({ "jsonrpc": "2.0", "id": 4, "method": "shutdown" })).await;
    let reply = wait_for(&mut ws, |m| m.get("id").is_some_and(|i| i == 3 || i == 4)).await;
    assert_eq!(reply["id"], 4);
}
