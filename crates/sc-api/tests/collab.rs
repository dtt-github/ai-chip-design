//! Co-editing over the collab WebSocket with `yrs` clients.

mod common;

use std::time::Duration;

use common::token;
use futures_util::{SinkExt, StreamExt};
use sc_api::lsp::LspConfig;
use sc_api::router;
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio_tungstenite::tungstenite::Message as WsMsg;
use yrs::encoding::read::Cursor;
use yrs::sync::{Message, MessageReader, SyncMessage};
use yrs::updates::decoder::{Decode, DecoderV1};
use yrs::updates::encoder::Encode;
use yrs::{Doc, GetString, Map, Out, ReadTxn, Text, Transact, Update};

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn serve(db: PgPool) -> (String, tempfile::TempDir) {
    let (state, dir) = common::state(db, LspConfig::disabled());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    tokio::spawn(async move { axum::serve(listener, router(state)).await.unwrap() });
    (addr, dir)
}

async fn http(addr: &str, tok: &str, method: reqwest::Method, path: &str, body: Value) -> Value {
    let res = reqwest::Client::new()
        .request(method, format!("http://{addr}/api{path}"))
        .bearer_auth(tok)
        .json(&body)
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "{path}: {}", res.status());
    res.json().await.unwrap()
}

struct Client {
    ws: Ws,
    doc: Doc,
    role: String,
}

impl Client {
    async fn connect(addr: &str, project: &str, tok: &str) -> Client {
        let (mut ws, _) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/api/projects/{project}/collab")).await.unwrap();
        ws.send(WsMsg::text(json!({ "token": tok }).to_string())).await.unwrap();
        let hello: Value = match ws.next().await.unwrap().unwrap() {
            WsMsg::Text(t) => serde_json::from_str(t.as_str()).unwrap(),
            m => panic!("{m:?}"),
        };
        assert_eq!(hello["type"], "ready", "{hello}");
        let mut c = Client { ws, doc: Doc::new(), role: hello["role"].as_str().unwrap().into() };
        // Ask for the full state, like y-websocket does.
        let sv = c.doc.transact().state_vector();
        c.send(Message::Sync(SyncMessage::SyncStep1(sv))).await;
        c.pump(Duration::from_millis(300)).await;
        c
    }

    async fn send(&mut self, m: Message) {
        self.ws.send(WsMsg::binary(m.encode_v1())).await.unwrap();
    }

    /// Applies incoming messages for `dur`, answering sync step 1.
    async fn pump(&mut self, dur: Duration) {
        let deadline = tokio::time::Instant::now() + dur;
        while let Ok(Some(Ok(msg))) = tokio::time::timeout_at(deadline, self.ws.next()).await {
            let WsMsg::Binary(data) = msg else { continue };
            let mut dec = DecoderV1::new(Cursor::new(&data));
            let msgs: Vec<Message> = MessageReader::new(&mut dec).map(|m| m.unwrap()).collect();
            for m in msgs {
                match m {
                    Message::Sync(SyncMessage::SyncStep1(sv)) => {
                        let u = self.doc.transact().encode_state_as_update_v1(&sv);
                        self.send(Message::Sync(SyncMessage::SyncStep2(u))).await;
                    }
                    Message::Sync(SyncMessage::SyncStep2(u) | SyncMessage::Update(u)) => {
                        self.doc.transact_mut().apply_update(Update::decode_v1(&u).unwrap()).unwrap();
                    }
                    _ => {}
                }
            }
        }
    }

    fn text(&self, path: &str) -> Option<String> {
        let files = self.doc.get_or_insert_map("files");
        let txn = self.doc.transact();
        match files.get(&txn, path) {
            Some(Out::YText(t)) => Some(t.get_string(&txn)),
            _ => None,
        }
    }

    fn head(&self) -> Option<String> {
        let meta = self.doc.get_or_insert_map("meta");
        let txn = self.doc.transact();
        match meta.get(&txn, "head") {
            Some(Out::Any(yrs::Any::String(s))) => Some(s.to_string()),
            _ => None,
        }
    }

    /// Inserts at the start of a file and sends the update.
    async fn prepend(&mut self, path: &str, s: &str) {
        let files = self.doc.get_or_insert_map("files");
        let update = {
            let mut txn = self.doc.transact_mut();
            let Some(Out::YText(t)) = files.get(&txn, path) else { panic!("no {path}") };
            t.insert(&mut txn, 0, s);
            txn.encode_update_v1()
        };
        self.send(Message::Sync(SyncMessage::Update(update))).await;
    }
}

#[sqlx::test(migrator = "sc_api::MIGRATOR")]
async fn two_editors_a_viewer_and_a_commit(db: PgPool) {
    let (addr, _dir) = serve(db).await;
    let ada = token("ada", "ada@example.org");
    let bob = token("bob", "bob@example.org");
    let eve = token("eve", "eve@example.org");
    let p = http(&addr, &ada, reqwest::Method::POST, "/projects", json!({ "name": "Shared" })).await;
    let id = p["id"].as_str().unwrap().to_string();
    let head = p["head"].as_str().unwrap().to_string();
    for (who, role) in [(&bob, "editor"), (&eve, "viewer")] {
        http(&addr, who, reqwest::Method::GET, "/me", Value::Null).await;
        let email = if who == &bob { "bob@example.org" } else { "eve@example.org" };
        http(
            &addr,
            &ada,
            reqwest::Method::POST,
            &format!("/projects/{id}/members"),
            json!({ "email": email, "role": role }),
        )
        .await;
    }

    let mut a = Client::connect(&addr, &id, &ada).await;
    let mut b = Client::connect(&addr, &id, &bob).await;
    let mut v = Client::connect(&addr, &id, &eve).await;
    assert_eq!((a.role.as_str(), v.role.as_str()), ("editor", "viewer"));

    // Seeded from the initial commit.
    assert_eq!(a.head(), Some(head.clone()));
    let rtl = a.text("src/shared.v").expect("seeded");
    assert!(rtl.contains("module shared"));
    assert_eq!(b.text("README.md"), a.text("README.md"));

    // Ada types; Bob and the viewer see it.
    a.prepend("src/shared.v", "// ada was here\n").await;
    b.pump(Duration::from_millis(300)).await;
    v.pump(Duration::from_millis(100)).await;
    assert!(b.text("src/shared.v").unwrap().starts_with("// ada was here\n"));
    assert!(v.text("src/shared.v").unwrap().starts_with("// ada was here\n"));

    // The viewer's edits stay local.
    v.prepend("README.md", "vandalism\n").await;
    a.pump(Duration::from_millis(300)).await;
    assert!(!a.text("README.md").unwrap().contains("vandalism"));

    // A commit of README.md (unedited in the room) flows into the shared copy;
    // Ada's uncommitted edit to the RTL stays.
    let c = http(
        &addr,
        &bob,
        reqwest::Method::POST,
        &format!("/projects/{id}/commits"),
        json!({ "message": "readme", "changes": [{ "op": "write", "path": "README.md", "content": "# Shared\n" }] }),
    )
    .await;
    a.pump(Duration::from_millis(300)).await;
    assert_eq!(a.head().as_deref(), c["id"].as_str());
    assert_eq!(a.text("README.md").as_deref(), Some("# Shared\n"));
    assert!(a.text("src/shared.v").unwrap().starts_with("// ada was here\n"));

    // Everyone leaves; the working copy survives and comes back.
    drop((a, b, v));
    tokio::time::sleep(Duration::from_millis(300)).await;
    let a2 = Client::connect(&addr, &id, &ada).await;
    assert!(a2.text("src/shared.v").unwrap().starts_with("// ada was here\n"));
    assert_eq!(a2.text("README.md").as_deref(), Some("# Shared\n"));
}

#[sqlx::test(migrator = "sc_api::MIGRATOR")]
async fn outsiders_are_refused(db: PgPool) {
    let (addr, _dir) = serve(db).await;
    let ada = token("ada", "ada@example.org");
    let p = http(&addr, &ada, reqwest::Method::POST, "/projects", json!({ "name": "Private" })).await;
    let id = p["id"].as_str().unwrap();
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/api/projects/{id}/collab")).await.unwrap();
    ws.send(WsMsg::text(json!({ "token": token("mallory", "m@example.org") }).to_string())).await.unwrap();
    let WsMsg::Text(t) = ws.next().await.unwrap().unwrap() else { panic!() };
    let v: Value = serde_json::from_str(t.as_str()).unwrap();
    assert_eq!(v["error"], "not_found");
}
