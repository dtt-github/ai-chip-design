//! Real-time co-editing: one Yjs document ("room") per project.
//!
//! The document holds the project's working copy shared by everyone who has
//! it open:
//! - `files`: map of path → `Y.Text` for every text file,
//! - `deleted`: map of path → `true` for deletions staged in the working copy,
//! - `meta.head`: the commit the working copy is based on.
//!
//! `GET /api/projects/{id}/collab` is a WebSocket. The client first sends
//! `{"token": "..."}`, gets `{"type":"ready","role":...}`, then exchanges
//! binary y-protocols messages (sync step 1/2, updates, awareness) — the same
//! wire format as y-websocket. Viewers receive updates but their document
//! changes are dropped.
//!
//! Rooms are created on first connect (from a saved state if there is one,
//! else seeded from the branch tip), saved to `<data>/collab/<id>.ydoc`, and
//! dropped when the last client leaves. After every commit the room is
//! reconciled with Git: files nobody has edited take the committed content.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Weak};
use std::time::Duration;

use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{Path as UrlPath, State};
use axum::response::Response;
use serde_json::json;
use tokio::sync::{Mutex, broadcast};
use uuid::Uuid;
use yrs::encoding::read::Cursor;
use yrs::sync::{Awareness, Message, MessageReader, SyncMessage};
use yrs::updates::decoder::{Decode, DecoderV1};
use yrs::updates::encoder::{Encode, Encoder, EncoderV1};
use yrs::{
    Doc, GetString, Map, MapRef, OffsetKind, Options, Out, ReadTxn, StateVector, Text, TextPrelim, Transact, Update,
};

use crate::AppState;
use crate::auth::authenticate;
use crate::projects::{Role, authorize};

/// Largest file put into the shared document; bigger or binary files stay Git-only.
const MAX_TEXT_FILE: usize = 1024 * 1024;
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
const SAVE_INTERVAL: Duration = Duration::from_secs(3);

pub struct Collab {
    dir: PathBuf,
    rooms: Mutex<HashMap<Uuid, Arc<Room>>>,
}

pub struct Room {
    project: Uuid,
    state: Mutex<RoomState>,
    /// (sender connection, encoded message) to every connection in the room.
    tx: broadcast::Sender<(u64, Arc<Vec<u8>>)>,
    /// People whose access was removed: their connections close.
    kick: broadcast::Sender<Uuid>,
}

struct RoomState {
    awareness: Awareness,
    conns: usize,
    next_conn: u64,
    dirty: bool,
}

impl Collab {
    pub fn new(dir: PathBuf) -> Self {
        Collab { dir, rooms: Mutex::new(HashMap::new()) }
    }

    fn file(&self, project: Uuid) -> PathBuf {
        self.dir.join(format!("{project}.ydoc"))
    }

    /// Closes `user`'s connections to the project's room (after they lose access).
    pub async fn disconnect(&self, project: Uuid, user: Uuid) {
        if let Some(room) = self.live(project).await {
            let _ = room.kick.send(user);
        }
    }

    /// The live room for a project, if anyone has it open.
    pub async fn live(&self, project: Uuid) -> Option<Arc<Room>> {
        self.rooms.lock().await.get(&project).cloned()
    }

    async fn join(self: &Arc<Self>, state: &AppState, project: Uuid) -> anyhow::Result<(Arc<Room>, u64)> {
        let mut rooms = self.rooms.lock().await;
        let room = match rooms.get(&project) {
            Some(r) => r.clone(),
            None => {
                let doc = self.load_or_seed(state, project).await?;
                let (tx, _) = broadcast::channel(1024);
                let room = Arc::new(Room {
                    project,
                    state: Mutex::new(RoomState {
                        awareness: Awareness::new(doc),
                        conns: 0,
                        next_conn: 1,
                        dirty: false,
                    }),
                    tx,
                    kick: broadcast::channel(16).0,
                });
                rooms.insert(project, room.clone());
                tokio::spawn(saver(Arc::downgrade(self), Arc::downgrade(&room)));
                room
            }
        };
        let conn = {
            let mut st = room.state.lock().await;
            st.conns += 1;
            st.next_conn += 1;
            st.next_conn
        };
        Ok((room, conn))
    }

    async fn leave(&self, room: &Arc<Room>, clients: &HashSet<yrs::block::ClientID>) {
        let mut rooms = self.rooms.lock().await;
        let mut st = room.state.lock().await;
        st.conns -= 1;
        // Drop the leaving user's cursors for everyone else.
        if !clients.is_empty() {
            for &c in clients {
                st.awareness.remove_state(c);
            }
            if let Ok(u) = st.awareness.update_with_clients(clients.iter().copied()) {
                let _ = room.tx.send((0, Arc::new(Message::Awareness(u).encode_v1())));
            }
        }
        if st.conns == 0 {
            if let Err(e) = self.save(room.project, &st.awareness.doc().clone()).await {
                tracing::error!(project = %room.project, error = ?e, "saving collab state failed");
            }
            rooms.remove(&room.project);
            tracing::info!(project = %room.project, "collab room closed");
        }
    }

    async fn save(&self, project: Uuid, doc: &Doc) -> anyhow::Result<()> {
        let bytes = doc.transact().encode_state_as_update_v1(&StateVector::default());
        tokio::fs::create_dir_all(&self.dir).await?;
        let tmp = self.file(project).with_extension("tmp");
        tokio::fs::write(&tmp, bytes).await?;
        tokio::fs::rename(tmp, self.file(project)).await?;
        Ok(())
    }

    async fn load_or_seed(&self, state: &AppState, project: Uuid) -> anyhow::Result<Doc> {
        let doc = Doc::with_options(Options { offset_kind: OffsetKind::Utf16, ..Options::default() });
        let head = crate::workspace::head(state.repos.clone(), project).await?;
        match tokio::fs::read(self.file(project)).await {
            Ok(bytes) => {
                doc.transact_mut().apply_update(Update::decode_v1(&bytes)?)?;
                let saved_head = meta_head(&doc);
                if let (Some(old), Some(new)) = (saved_head, head.clone())
                    && old != new
                {
                    reconcile(state, project, &doc, Some(&old), &new).await?;
                }
            }
            Err(_) => {
                if let Some(head) = head {
                    reconcile(state, project, &doc, None, &head).await?;
                }
            }
        }
        Ok(doc)
    }

    /// After a commit: bring the live room (if any) up to `new_head` and broadcast the change.
    pub async fn committed(&self, state: &AppState, project: Uuid, new_head: &str) {
        let Some(room) = self.live(project).await else { return };
        let mut st = room.state.lock().await;
        let doc = st.awareness.doc().clone();
        let before = doc.transact().state_vector();
        let old = meta_head(&doc);
        if let Err(e) = reconcile(state, project, &doc, old.as_deref(), new_head).await {
            tracing::error!(%project, error = ?e, "reconciling collab state after commit failed");
            return;
        }
        let update = doc.transact().encode_state_as_update_v1(&before);
        st.dirty = true;
        let _ = room.tx.send((0, Arc::new(Message::Sync(SyncMessage::Update(update)).encode_v1())));
    }
}

fn meta_head(doc: &Doc) -> Option<String> {
    let meta = doc.get_or_insert_map("meta");
    let txn = doc.transact();
    match meta.get(&txn, "head") {
        Some(Out::Any(yrs::Any::String(s))) => Some(s.to_string()),
        _ => None,
    }
}

fn text_of(map: &MapRef, txn: &impl ReadTxn, path: &str) -> Option<String> {
    match map.get(txn, path) {
        Some(Out::YText(t)) => Some(t.get_string(txn)),
        _ => None,
    }
}

/// Moves the working copy from `old` (None: empty) to `new`. A file the room
/// has not changed since `old` takes its content at `new`; edited files keep
/// the room's version (they show up as changes against the new head).
async fn reconcile(state: &AppState, project: Uuid, doc: &Doc, old: Option<&str>, new: &str) -> anyhow::Result<()> {
    let old_files = match old {
        Some(o) => crate::workspace::text_files(state.repos.clone(), project, o.to_string(), MAX_TEXT_FILE)
            .await
            .unwrap_or_default(),
        None => HashMap::new(),
    };
    let new_files = crate::workspace::text_files(state.repos.clone(), project, new.to_string(), MAX_TEXT_FILE).await?;
    let files = doc.get_or_insert_map("files");
    let deleted = doc.get_or_insert_map("deleted");
    let meta = doc.get_or_insert_map("meta");
    let mut txn = doc.transact_mut();
    let paths: HashSet<&String> = old_files.keys().chain(new_files.keys()).collect();
    for path in paths {
        let current = text_of(&files, &txn, path);
        let was = old_files.get(path);
        let now = new_files.get(path);
        let untouched = match (&current, was) {
            (Some(c), Some(w)) => c == w,
            (None, None) => true,
            // Deleted in the room since `old`: keep it deleted unless the file is new at `new`.
            (None, Some(_)) => deleted.get(&txn, path).is_none(),
            (Some(_), None) => false,
        };
        if !untouched {
            continue;
        }
        match now {
            Some(content) => match files.get(&txn, path) {
                Some(Out::YText(t)) => {
                    let len = t.len(&txn);
                    t.remove_range(&mut txn, 0, len);
                    t.insert(&mut txn, 0, content);
                }
                _ => {
                    files.insert(&mut txn, path.as_str(), TextPrelim::new(content.as_str()));
                }
            },
            None => {
                files.remove(&mut txn, path);
            }
        }
    }
    // Staged deletions of files that no longer exist at the new head are done.
    let gone: Vec<String> = deleted.keys(&txn).filter(|p| !new_files.contains_key(*p)).map(str::to_string).collect();
    for p in gone {
        deleted.remove(&mut txn, &p);
    }
    meta.insert(&mut txn, "head", new.to_string());
    Ok(())
}

/// Saves dirty rooms every few seconds until the room closes.
async fn saver(collab: Weak<Collab>, room: Weak<Room>) {
    loop {
        tokio::time::sleep(SAVE_INTERVAL).await;
        let (Some(collab), Some(room)) = (collab.upgrade(), room.upgrade()) else { return };
        let doc = {
            let mut st = room.state.lock().await;
            if !st.dirty {
                continue;
            }
            st.dirty = false;
            st.awareness.doc().clone()
        };
        if let Err(e) = collab.save(room.project, &doc).await {
            tracing::error!(project = %room.project, error = ?e, "saving collab state failed");
        }
    }
}

pub async fn handler(ws: WebSocketUpgrade, State(state): State<AppState>, UrlPath(id): UrlPath<Uuid>) -> Response {
    ws.max_message_size(8 * 1024 * 1024).on_upgrade(move |socket| async move {
        if let Err(e) = session(state, id, socket).await {
            tracing::warn!(project = %id, error = ?e, "collab session ended with error");
        }
    })
}

async fn refuse(mut socket: WebSocket, code: &str, message: String) -> anyhow::Result<()> {
    let msg = json!({ "type": "error", "error": code, "message": message }).to_string();
    socket.send(WsMessage::Text(msg.into())).await?;
    let _ = socket.send(WsMessage::Close(None)).await;
    Ok(())
}

async fn session(state: AppState, project: Uuid, mut socket: WebSocket) -> anyhow::Result<()> {
    let token = match tokio::time::timeout(HELLO_TIMEOUT, socket.recv()).await {
        Ok(Some(Ok(WsMessage::Text(t)))) => serde_json::from_str::<serde_json::Value>(t.as_str())
            .ok()
            .and_then(|v| v.get("token").and_then(|t| t.as_str()).map(str::to_string)),
        _ => None,
    };
    let Some(token) = token else {
        return refuse(socket, "bad_request", "first message must be {\"token\": ...}".into()).await;
    };
    let user = match authenticate(&state, &token).await {
        Ok(u) => u,
        Err(e) => return refuse(socket, "unauthorized", e.to_string()).await,
    };
    let role = match authorize(&state, &user, project, Role::Viewer).await {
        Ok((_, role)) => role,
        Err(e) => return refuse(socket, "not_found", e.to_string()).await,
    };
    let can_edit = role.is_some_and(|r| r >= Role::Editor);
    let (room, conn) = state.collab.join(&state, project).await?;
    let mut rx = room.tx.subscribe();
    let mut kicked = room.kick.subscribe();
    let role_name = if can_edit { "editor" } else { "viewer" };
    socket.send(WsMessage::Text(json!({ "type": "ready", "role": role_name }).to_string().into())).await?;
    tracing::info!(%project, user = %user.id, role = role_name, "collab session started");

    // Like y-websocket: start with our state vector and everyone's cursors.
    let hello = {
        let st = room.state.lock().await;
        let mut enc = EncoderV1::new();
        Message::Sync(SyncMessage::SyncStep1(st.awareness.doc().transact().state_vector())).encode(&mut enc);
        if let Ok(aw) = st.awareness.update() {
            Message::Awareness(aw).encode(&mut enc);
        }
        enc.to_vec()
    };
    socket.send(WsMessage::Binary(hello.into())).await?;

    let mut clients: HashSet<yrs::block::ClientID> = HashSet::new();
    let result: anyhow::Result<()> = async {
        loop {
            tokio::select! {
                incoming = socket.recv() => match incoming {
                    Some(Ok(WsMessage::Binary(data))) => {
                        let replies = handle(&room, conn, can_edit, &data, &mut clients).await?;
                        for r in replies {
                            socket.send(WsMessage::Binary(r.into())).await?;
                        }
                    }
                    Some(Ok(WsMessage::Close(_))) | None | Some(Err(_)) => return Ok(()),
                    Some(Ok(_)) => {}
                },
                k = kicked.recv() => {
                    if k.is_ok_and(|u| u == user.id) {
                        let _ = socket.send(WsMessage::Close(None)).await;
                        return Ok(());
                    }
                }
                out = rx.recv() => match out {
                    Ok((from, msg)) if from != conn => socket.send(WsMessage::Binary(msg.as_ref().clone().into())).await?,
                    Ok(_) => {}
                    // Too far behind: the client resyncs on reconnect.
                    Err(broadcast::error::RecvError::Lagged(_)) => return Ok(()),
                    Err(broadcast::error::RecvError::Closed) => return Ok(()),
                },
            }
        }
    }
    .await;
    state.collab.leave(&room, &clients).await;
    tracing::info!(%project, user = %user.id, "collab session closed");
    result
}

/// Handles one binary frame; returns replies for the sender.
async fn handle(
    room: &Room,
    conn: u64,
    can_edit: bool,
    data: &[u8],
    clients: &mut HashSet<yrs::block::ClientID>,
) -> anyhow::Result<Vec<Vec<u8>>> {
    let mut replies = Vec::new();
    let mut st = room.state.lock().await;
    let mut decoder = DecoderV1::new(Cursor::new(data));
    let reader = MessageReader::new(&mut decoder);
    for msg in reader {
        match msg? {
            Message::Sync(SyncMessage::SyncStep1(sv)) => {
                let update = st.awareness.doc().transact().encode_state_as_update_v1(&sv);
                replies.push(Message::Sync(SyncMessage::SyncStep2(update)).encode_v1());
            }
            Message::Sync(SyncMessage::SyncStep2(update) | SyncMessage::Update(update)) => {
                if !can_edit {
                    continue; // viewers can't change the working copy
                }
                st.awareness.doc().transact_mut().apply_update(Update::decode_v1(&update)?)?;
                st.dirty = true;
                let _ = room.tx.send((conn, Arc::new(Message::Sync(SyncMessage::Update(update)).encode_v1())));
            }
            Message::Awareness(update) => {
                clients.extend(update.clients.keys().copied());
                let encoded = Message::Awareness(update.clone()).encode_v1();
                st.awareness.apply_update(update)?;
                let _ = room.tx.send((conn, Arc::new(encoded)));
            }
            Message::AwarenessQuery => {
                if let Ok(u) = st.awareness.update() {
                    replies.push(Message::Awareness(u).encode_v1());
                }
            }
            Message::Auth(_) | Message::Custom(..) => {}
        }
    }
    Ok(replies)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meta_head_round_trip() {
        let doc = Doc::new();
        assert_eq!(meta_head(&doc), None);
        let meta = doc.get_or_insert_map("meta");
        meta.insert(&mut doc.transact_mut(), "head", "abc".to_string());
        assert_eq!(meta_head(&doc).as_deref(), Some("abc"));
    }
}
