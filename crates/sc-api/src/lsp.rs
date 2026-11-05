//! WebSocket bridge to the Verible Verilog language server.
//!
//! Protocol on `GET /api/projects/{id}/lsp` (WebSocket):
//! 1. The client sends `{"token": "<access token>"}` (browsers cannot set
//!    headers on WebSocket requests, and tokens don't belong in URLs).
//! 2. The server answers `{"type": "ready"}` or `{"type": "error", ...}`.
//! 3. Then every text frame is one JSON-RPC message, in both directions.
//!
//! Each session gets a temporary workspace holding the project's files at the
//! branch tip plus a `verible.filelist`, and its own `verible-verilog-ls`
//! process. Clients name documents `file:///<project id>/<path>`; the bridge
//! rewrites those to the workspace and back, and drops any message that refers
//! to another `file:` URI so the server can't be pointed at the host's files.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path as UrlPath, State};
use axum::response::Response;
use serde_json::{Value, json};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::{Semaphore, mpsc};
use uuid::Uuid;

use crate::AppState;
use crate::auth::authenticate;
use crate::projects::{Role, authorize};

const MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_WORKSPACE_FILES: usize = 5000;
const HDL_EXTENSIONS: [&str; 4] = ["v", "vh", "sv", "svh"];

pub struct LspConfig {
    /// Path to `verible-verilog-ls`; `None` disables the endpoint.
    pub verible_ls: Option<PathBuf>,
    pub sessions: Arc<Semaphore>,
}

impl LspConfig {
    pub fn new(verible_ls: Option<PathBuf>, max_sessions: usize) -> Self {
        Self { verible_ls, sessions: Arc::new(Semaphore::new(max_sessions)) }
    }

    pub fn disabled() -> Self {
        Self::new(None, 0)
    }
}

pub async fn handler(ws: WebSocketUpgrade, State(state): State<AppState>, UrlPath(id): UrlPath<Uuid>) -> Response {
    ws.max_message_size(MAX_MESSAGE_BYTES).on_upgrade(move |socket| async move {
        if let Err(e) = session(state, id, socket).await {
            tracing::warn!(project = %id, error = ?e, "lsp session ended with error");
        }
    })
}

async fn send_json(socket: &mut WebSocket, v: Value) -> anyhow::Result<()> {
    socket.send(Message::Text(v.to_string().into())).await?;
    Ok(())
}

async fn refuse(mut socket: WebSocket, code: &str, message: impl Into<String>) -> anyhow::Result<()> {
    send_json(&mut socket, json!({ "type": "error", "error": code, "message": message.into() })).await?;
    let _ = socket.send(Message::Close(None)).await;
    Ok(())
}

async fn session(state: AppState, id: Uuid, mut socket: WebSocket) -> anyhow::Result<()> {
    let token = match tokio::time::timeout(HELLO_TIMEOUT, socket.recv()).await {
        Ok(Some(Ok(Message::Text(t)))) => serde_json::from_str::<Value>(t.as_str())
            .ok()
            .and_then(|v| v.get("token").and_then(Value::as_str).map(str::to_string)),
        _ => None,
    };
    let Some(token) = token else {
        return refuse(socket, "bad_request", "first message must be {\"token\": ...}").await;
    };
    let user = match authenticate(&state, &token).await {
        Ok(u) => u,
        Err(e) => return refuse(socket, "unauthorized", e.to_string()).await,
    };
    if let Err(e) = authorize(&state, &user, id, Role::Viewer).await {
        return refuse(socket, "not_found", e.to_string()).await;
    }
    let Some(bin) = state.lsp.verible_ls.clone() else {
        return refuse(socket, "unavailable", "the Verilog language server is not installed on this server").await;
    };
    let Ok(_permit) = state.lsp.sessions.clone().try_acquire_owned() else {
        return refuse(socket, "busy", "too many language server sessions; try again later").await;
    };

    let workspace = tempfile::Builder::new().prefix("sc-lsp-").tempdir()?;
    let hdl_files = materialize(&state, id, workspace.path().to_path_buf()).await?;

    let mut child = Command::new(&bin)
        .current_dir(workspace.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let mut stdin = child.stdin.take().expect("piped stdin");
    let stdout = BufReader::new(child.stdout.take().expect("piped stdout"));
    tracing::info!(project = %id, user = %user.id, files = hdl_files, "lsp session started");

    let rewriter = UriRewriter::new(id, workspace.path());
    let (out_tx, mut out_rx) = mpsc::channel::<Vec<u8>>(64);
    let reader = tokio::spawn(async move {
        let mut stdout = stdout;
        while let Ok(Some(msg)) = read_message(&mut stdout).await {
            if out_tx.send(msg).await.is_err() {
                break;
            }
        }
    });

    send_json(&mut socket, json!({ "type": "ready" })).await?;
    loop {
        tokio::select! {
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Text(text))) => {
                    let Ok(mut msg) = serde_json::from_str::<Value>(text.as_str()) else { continue };
                    if !rewriter.to_server(&mut msg) {
                        tracing::warn!(project = %id, "dropped lsp message with a foreign file URI");
                        continue;
                    }
                    if msg.get("method").and_then(Value::as_str) == Some("initialize") {
                        rewriter.set_root(&mut msg);
                    }
                    write_message(&mut stdin, &msg).await?;
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(_)) => {}
            },
            outgoing = out_rx.recv() => match outgoing {
                Some(bytes) => {
                    let Ok(mut msg) = serde_json::from_slice::<Value>(&bytes) else { continue };
                    rewriter.to_client(&mut msg);
                    send_json(&mut socket, msg).await?;
                }
                // The language server exited.
                None => {
                    let _ = socket.send(Message::Close(None)).await;
                    break;
                }
            },
        }
    }
    reader.abort();
    let _ = child.kill().await;
    tracing::info!(project = %id, "lsp session closed");
    Ok(())
}

/// Writes the project's files at the branch tip into `dir`, plus a
/// `verible.filelist` naming the HDL sources. Returns the HDL file count.
async fn materialize(state: &AppState, id: Uuid, dir: PathBuf) -> anyhow::Result<usize> {
    let repos = state.repos.clone();
    let count = tokio::task::spawn_blocking(move || -> anyhow::Result<usize> {
        let repo = repos.open(&id.to_string())?;
        if repo.head()?.is_none() {
            return Ok(0);
        }
        let mut hdl = Vec::new();
        for entry in repo.list_files(None)?.into_iter().take(MAX_WORKSPACE_FILES) {
            let target = dir.join(&entry.path);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&target, repo.read_file(None, &entry.path)?)?;
            if is_hdl(&entry.path) {
                hdl.push(entry.path);
            }
        }
        let mut list = hdl.join("\n");
        list.push('\n');
        std::fs::write(dir.join("verible.filelist"), list)?;
        Ok(hdl.len())
    })
    .await??;
    Ok(count)
}

fn is_hdl(path: &str) -> bool {
    path.rsplit_once('.').is_some_and(|(_, ext)| HDL_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
}

/// Maps between the client's `file:///<project id>/...` URIs and the workspace.
struct UriRewriter {
    client_prefix: String,
    server_prefix: String,
}

impl UriRewriter {
    fn new(project: Uuid, workspace: &Path) -> Self {
        Self { client_prefix: format!("file:///{project}/"), server_prefix: format!("file://{}/", workspace.display()) }
    }

    /// Rewrites client URIs in place. Returns `false` if the message names a
    /// `file:` URI outside the project, in which case it must be dropped.
    fn to_server(&self, msg: &mut Value) -> bool {
        let mut ok = true;
        walk_strings(msg, &mut |s| {
            if let Some(rest) = s.strip_prefix(&self.client_prefix) {
                if rest.split('/').any(|seg| seg == ".." || seg == "%2E%2E" || seg == "%2e%2e") {
                    ok = false;
                }
                *s = format!("{}{rest}", self.server_prefix);
            } else if s.starts_with("file:/") {
                ok = false;
            }
        });
        ok
    }

    fn to_client(&self, msg: &mut Value) {
        walk_strings(msg, &mut |s| {
            if let Some(rest) = s.strip_prefix(&self.server_prefix) {
                *s = format!("{}{rest}", self.client_prefix);
            }
        });
    }

    /// Points `initialize` at the workspace, whatever the client sent.
    fn set_root(&self, msg: &mut Value) {
        let root_uri = self.server_prefix.trim_end_matches('/').to_string();
        let root_path = root_uri.trim_start_matches("file://").to_string();
        if let Some(params) = msg.get_mut("params").and_then(Value::as_object_mut) {
            params.insert("rootUri".into(), json!(root_uri));
            params.insert("rootPath".into(), json!(root_path));
            params.insert("workspaceFolders".into(), json!([{ "uri": root_uri, "name": "project" }]));
        }
    }
}

fn walk_strings(v: &mut Value, f: &mut impl FnMut(&mut String)) {
    match v {
        Value::String(s) => f(s),
        Value::Array(a) => a.iter_mut().for_each(|x| walk_strings(x, f)),
        Value::Object(o) => o.values_mut().for_each(|x| walk_strings(x, f)),
        _ => {}
    }
}

/// Reads one `Content-Length`-framed LSP message. `Ok(None)` on EOF.
async fn read_message<R: AsyncBufRead + Unpin>(r: &mut R) -> std::io::Result<Option<Vec<u8>>> {
    let invalid = |m: &str| std::io::Error::new(std::io::ErrorKind::InvalidData, m.to_string());
    let mut len = None;
    let mut line = String::new();
    loop {
        line.clear();
        if r.read_line(&mut line).await? == 0 {
            return Ok(None);
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            len = Some(value.trim().parse::<usize>().map_err(|_| invalid("bad Content-Length"))?);
        }
    }
    let len = len.ok_or_else(|| invalid("missing Content-Length"))?;
    if len > MAX_MESSAGE_BYTES {
        return Err(invalid("message too large"));
    }
    let mut buf = vec![0; len];
    r.read_exact(&mut buf).await?;
    Ok(Some(buf))
}

async fn write_message<W: AsyncWrite + Unpin>(w: &mut W, msg: &Value) -> std::io::Result<()> {
    let body = msg.to_string();
    w.write_all(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes()).await?;
    w.write_all(body.as_bytes()).await?;
    w.flush().await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rewriter() -> (Uuid, UriRewriter) {
        let id = Uuid::nil();
        (id, UriRewriter::new(id, Path::new("/tmp/sc-lsp-x")))
    }

    #[test]
    fn rewrites_both_ways() {
        let (_, rw) = rewriter();
        let mut msg = json!({"params": {"textDocument": {"uri": "file:///00000000-0000-0000-0000-000000000000/src/a.v", "text": "file:/not/a/uri"}}});
        // Any string that looks like a file URI is checked, even document text: better safe than sorry.
        assert!(!rw.to_server(&mut msg.clone()));
        msg["params"]["textDocument"]["text"] = json!("module a; endmodule");
        assert!(rw.to_server(&mut msg));
        assert_eq!(msg["params"]["textDocument"]["uri"], "file:///tmp/sc-lsp-x/src/a.v");
        rw.to_client(&mut msg);
        assert_eq!(msg["params"]["textDocument"]["uri"], "file:///00000000-0000-0000-0000-000000000000/src/a.v");
    }

    #[test]
    fn refuses_foreign_and_escaping_uris() {
        let (id, rw) = rewriter();
        assert!(!rw.to_server(&mut json!({"uri": "file:///etc/passwd"})));
        assert!(!rw.to_server(&mut json!({"uri": format!("file:///{id}/../../etc/passwd")})));
        assert!(!rw.to_server(&mut json!({"uri": format!("file:///{}/a.v", Uuid::max())})));
        assert!(rw.to_server(&mut json!({"uri": "untitled:1", "n": 3})));
    }

    #[test]
    fn initialize_points_at_workspace() {
        let (_, rw) = rewriter();
        let mut msg = json!({"method": "initialize", "params": {"rootUri": null}});
        rw.set_root(&mut msg);
        assert_eq!(msg["params"]["rootUri"], "file:///tmp/sc-lsp-x");
        assert_eq!(msg["params"]["workspaceFolders"][0]["uri"], "file:///tmp/sc-lsp-x");
    }

    #[tokio::test]
    async fn framing_round_trip() {
        let mut buf = Vec::new();
        write_message(&mut buf, &json!({"jsonrpc": "2.0", "id": 1})).await.unwrap();
        write_message(&mut buf, &json!({"x": "é"})).await.unwrap();
        let mut r = BufReader::new(&buf[..]);
        assert_eq!(read_message(&mut r).await.unwrap().unwrap(), br#"{"id":1,"jsonrpc":"2.0"}"#);
        assert_eq!(read_message(&mut r).await.unwrap().unwrap(), "{\"x\":\"é\"}".as_bytes());
        assert!(read_message(&mut r).await.unwrap().is_none());
    }

    #[test]
    fn hdl_detection() {
        assert!(is_hdl("src/a.v") && is_hdl("x.SV") && is_hdl("inc/defs.svh"));
        assert!(!is_hdl("config.json") && !is_hdl("Makefile"));
    }
}
