//! Private per-terminal Codex `app-server` launcher and WebSocket-over-UDS client (ADR-0105).

#[cfg(test)]
pub(super) mod tests;
mod ws;

use std::collections::{HashMap, HashSet};
use std::fs;
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};

use super::session::Session;
use crate::data::boss_app::boss_key;

const RPC_TIMEOUT: Duration = Duration::from_secs(4);
const CONNECT_POLL: Duration = Duration::from_millis(100);

/// Runs a private `codex app-server` in the terminal's own process tree, waits for its Unix
/// socket, then runs the `--remote` TUI in the foreground without permission overrides.
const WRAPPER_SCRIPT: &str = concat!(
    "codex=\"$1\"; sock=\"$2\"; cwd=\"$3\"; resume_id=\"$4\"; shift 4\n",
    "rm -f \"$sock\"\n",
    "\"$codex\" app-server --listen \"unix://$sock\" -c check_for_update_on_startup=false \"$@\" >/dev/null 2>&1 &\n",
    "app=$!\n",
    "cleanup() { kill \"$app\" 2>/dev/null || true; rm -f \"$sock\"; }\n",
    "trap cleanup EXIT HUP INT TERM\n",
    "i=0\n",
    "while [ \"$i\" -lt 150 ]; do\n",
    "  [ -S \"$sock\" ] && break\n",
    "  if ! kill -0 \"$app\" 2>/dev/null; then wait \"$app\"; exit $?; fi\n",
    "  sleep 0.1; i=$((i + 1))\n",
    "done\n",
    "if [ ! -S \"$sock\" ]; then echo \"codex app-server socket timeout: $sock\" >&2; exit 1; fi\n",
    "if [ -n \"$resume_id\" ]; then\n",
    "  \"$codex\" resume --remote \"unix://$sock\" -C \"$cwd\" -c check_for_update_on_startup=false \"$@\" \"$resume_id\"\n",
    "else\n",
    "  \"$codex\" --remote \"unix://$sock\" -C \"$cwd\" -c check_for_update_on_startup=false \"$@\"\n",
    "fi\n",
);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ThreadStatus {
    Idle,
    Active { waiting_on_approval: bool },
    NotLoaded,
    SystemError,
    Unknown,
}

impl ThreadStatus {
    fn from_value(value: &Value) -> Self {
        match value["type"].as_str() {
            Some("idle") => Self::Idle,
            Some("notLoaded") => Self::NotLoaded,
            Some("systemError") => Self::SystemError,
            Some("active") => {
                let waiting_on_approval = value["activeFlags"].as_array().is_some_and(|flags| {
                    flags
                        .iter()
                        .any(|f| f.as_str() == Some("waitingOnApproval"))
                });
                Self::Active {
                    waiting_on_approval,
                }
            }
            _ => Self::Unknown,
        }
    }

    fn is_idle(&self) -> bool {
        matches!(self, Self::Idle | Self::SystemError)
    }

    fn is_deliverable(&self) -> bool {
        !matches!(
            self,
            Self::NotLoaded
                | Self::Active {
                    waiting_on_approval: true
                }
        )
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Delivery {
    Sent,
    NotReady,
    Failed,
}

#[derive(Default)]
struct ClientState {
    connected: bool,
    closed: bool,
    generation: u64,
    next_id: u64,
    thread_seq: u64,
    writer: Option<Arc<Mutex<UnixStream>>>,
    shutdown: Option<UnixStream>,
    pending: HashMap<u64, mpsc::Sender<Value>>,
    threads: HashMap<String, (ThreadStatus, u64)>,
    #[cfg(test)]
    last_error: Option<String>,
}

pub(super) struct Client {
    sock: PathBuf,
    state: Arc<Mutex<ClientState>>,
}

impl Drop for Client {
    fn drop(&mut self) {
        self.close();
    }
}

pub(super) fn socket_path(terminal_id: u64) -> PathBuf {
    let name = format!("an-cx-{}-{terminal_id}.sock", std::process::id());
    let candidate = std::env::temp_dir().join(&name);
    // sun_path limit is 104 bytes on macOS and 108 on Linux.
    if candidate.as_os_str().len() < 96 {
        candidate
    } else {
        PathBuf::from("/tmp").join(name)
    }
}

pub(super) fn launch_args(
    program: &Path,
    sock: &Path,
    cwd: &Path,
    resume: Option<&str>,
    extra: &[String],
) -> (PathBuf, Vec<String>) {
    let mut args = vec![
        "-c".to_owned(),
        WRAPPER_SCRIPT.to_owned(),
        "agentnave-codex".to_owned(),
        program.to_string_lossy().into_owned(),
        sock.to_string_lossy().into_owned(),
        cwd.to_string_lossy().into_owned(),
        resume.unwrap_or_default().to_owned(),
    ];
    args.extend(extra.iter().cloned());
    (PathBuf::from("/bin/sh"), args)
}

impl Client {
    pub(super) fn connect_in_background(sock: PathBuf, session: Arc<Session>) -> Arc<Self> {
        let client = Arc::new(Self {
            sock,
            state: Arc::default(),
        });
        let runner = client.clone();
        thread::spawn(move || runner.connect_loop(Some(session)));
        client
    }

    fn connect_loop(&self, session: Option<Arc<Session>>) {
        while !self.is_closed() && session.as_ref().is_none_or(|s| s.exit().is_none()) {
            if self.is_connected() {
                thread::sleep(Duration::from_millis(250));
                continue;
            }
            if !self.sock.exists() {
                thread::sleep(CONNECT_POLL);
                continue;
            }
            if let Err(err) = self.try_connect() {
                #[cfg(not(test))]
                let _ = err;
                #[cfg(test)]
                {
                    self.state
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .last_error = Some(err);
                }
                thread::sleep(CONNECT_POLL);
            }
        }
    }

    fn try_connect(&self) -> Result<(), String> {
        let gen_id = {
            let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            if state.closed {
                return Err("closed".to_owned());
            }
            state.generation += 1;
            state.connected = false;
            state.writer = None;
            state.pending.clear();
            state.threads.clear();
            state.generation
        };

        let (reader, writer) = ws::connect_with(&self.sock, |shutdown_stream| {
            let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            if state.closed || state.generation != gen_id {
                let _ = shutdown_stream.shutdown(Shutdown::Both);
                return Err("closed".to_owned());
            }
            state.shutdown = Some(shutdown_stream);
            Ok(())
        })?;

        {
            let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            if state.closed || state.generation != gen_id {
                let old_shutdown = state.shutdown.take();
                drop(state);
                if let Some(s) = old_shutdown {
                    let _ = s.shutdown(Shutdown::Both);
                }
                return Err("closed".to_owned());
            }
            state.writer = Some(writer);
        }
        let state_ref = self.state.clone();
        thread::spawn(move || read_loop(reader, state_ref, gen_id));

        let init = self.request(
            "initialize",
            Some(json!({
                "clientInfo": {
                    "name": "agentnave-tray",
                    "title": "AgentNave",
                    "version": env!("CARGO_PKG_VERSION"),
                },
                "capabilities": {
                    "experimentalApi": true,
                },
            })),
        );
        if !matches!(init, Ok(Ok(_))) {
            self.disconnect_generation(gen_id);
            return Err("initialize failed".to_owned());
        }
        if self.notify("initialized", None).is_err() {
            self.disconnect_generation(gen_id);
            return Err("initialized notify failed".to_owned());
        }
        {
            let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            if state.closed || state.generation != gen_id {
                return Err("stale connection".to_owned());
            }
            state.connected = true;
        }
        self.refresh_threads();
        Ok(())
    }

    fn disconnect_generation(&self, gen_id: u64) {
        let shutdown = {
            let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            if state.generation != gen_id {
                return;
            }
            state.connected = false;
            state.writer = None;
            state.pending.clear();
            state.threads.clear();
            state.shutdown.take()
        };
        if let Some(stream) = shutdown {
            let _ = stream.shutdown(Shutdown::Both);
        }
    }

    pub(super) fn close(&self) {
        let shutdown = {
            let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            state.closed = true;
            state.connected = false;
            state.writer = None;
            state.pending.clear();
            state.threads.clear();
            state.shutdown.take()
        };
        if let Some(stream) = shutdown {
            let _ = stream.shutdown(Shutdown::Both);
        }
        let _ = fs::remove_file(&self.sock);
    }

    fn is_closed(&self) -> bool {
        self.state.lock().unwrap_or_else(|p| p.into_inner()).closed
    }

    fn is_connected(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .connected
    }

    #[cfg(test)]
    pub(super) fn is_connected_for_test(&self) -> bool {
        self.is_connected()
    }

    #[cfg(test)]
    pub(super) fn loaded_thread_ids_for_test(&self) -> Vec<String> {
        self.refresh_threads();
        self.state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .threads
            .keys()
            .cloned()
            .collect()
    }

    #[cfg(test)]
    pub(super) fn debug_info_for_test(&self) -> (bool, bool, PathBuf, Option<String>) {
        let state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        (
            state.connected,
            self.sock.exists(),
            self.sock.clone(),
            state.last_error.clone(),
        )
    }

    #[cfg(test)]
    pub(super) fn thread_status_for_test(&self, tid: &str) -> Option<ThreadStatus> {
        self.state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .threads
            .get(tid)
            .map(|(status, _)| status.clone())
    }

    fn notify(&self, method: &str, params: Option<Value>) -> Result<(), String> {
        let writer = {
            let state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            state.writer.clone()
        }
        .ok_or_else(|| "not connected".to_owned())?;
        let payload = match params {
            Some(p) => json!({"jsonrpc": "2.0", "method": method, "params": p}),
            None => json!({"jsonrpc": "2.0", "method": method}),
        };
        ws::send_text(&writer, &payload.to_string())
    }

    fn request(&self, method: &str, params: Option<Value>) -> Result<Result<Value, Value>, String> {
        let (id, writer, rx) = {
            let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            let Some(writer) = state.writer.clone() else {
                return Err("not connected".to_owned());
            };
            state.next_id += 1;
            let id = state.next_id;
            let (tx, rx) = mpsc::channel();
            state.pending.insert(id, tx);
            (id, writer, rx)
        };
        let payload = match params {
            Some(p) => json!({"jsonrpc": "2.0", "id": id, "method": method, "params": p}),
            None => json!({"jsonrpc": "2.0", "id": id, "method": method}),
        };
        if let Err(err) = ws::send_text(&writer, &payload.to_string()) {
            self.state
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .pending
                .remove(&id);
            return Err(err);
        }
        let reply = match rx.recv_timeout(RPC_TIMEOUT) {
            Ok(msg) => msg,
            Err(_) => {
                self.state
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .pending
                    .remove(&id);
                return Err(format!("{method} timed out"));
            }
        };
        if let Some(error) = reply.get("error") {
            return Ok(Err(error.clone()));
        }
        Ok(Ok(reply["result"].clone()))
    }

    fn refresh_threads(&self) {
        let Ok(Ok(result)) = self.request("thread/loaded/list", Some(json!({}))) else {
            return;
        };
        let Some(ids) = result["data"].as_array() else {
            return;
        };
        let tids: Vec<String> = ids
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect();
        let loaded: HashSet<&str> = tids.iter().map(String::as_str).collect();
        {
            let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            state.threads.retain(|tid, _| loaded.contains(tid.as_str()));
        }
        for tid in tids {
            let seq_before = self
                .state
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .threads
                .get(&tid)
                .map(|(_, seq)| *seq);
            let status = match self.request(
                "thread/read",
                Some(json!({"threadId": &tid, "includeTurns": false})),
            ) {
                Ok(Ok(res)) => ThreadStatus::from_value(&res["thread"]["status"]),
                _ => ThreadStatus::Unknown,
            };
            let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            // Do not clobber a newer status notification received while `thread/read` was in flight.
            if state.threads.get(&tid).map(|(_, seq)| *seq) == seq_before {
                state.thread_seq += 1;
                let seq = state.thread_seq;
                state.threads.insert(tid, (status, seq));
            }
        }
    }

    fn resolve_thread(&self, target_boss_key: &str) -> Option<(String, ThreadStatus)> {
        {
            let state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            if !state.connected {
                return None;
            }
            if let Some((tid, (status, _))) = state
                .threads
                .iter()
                .find(|(tid, _)| boss_key("codex", tid) == target_boss_key)
            {
                return Some((tid.clone(), status.clone()));
            }
        }
        self.refresh_threads();
        let state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        state
            .threads
            .iter()
            .find(|(tid, _)| boss_key("codex", tid) == target_boss_key)
            .map(|(tid, (status, _))| (tid.clone(), status.clone()))
    }

    pub(super) fn deliver(
        &self,
        target_boss_key: &str,
        event_key: &str,
        message: &str,
    ) -> Delivery {
        let Some((tid, status)) = self.resolve_thread(target_boss_key) else {
            return Delivery::NotReady;
        };
        if !status.is_deliverable() {
            return Delivery::NotReady;
        }
        let input = json!([{"type": "text", "text": message, "text_elements": []}]);
        let queue_res = self.request(
            "thread/queue/add",
            Some(json!({
                "threadId": &tid,
                "input": &input,
                "clientUserMessageId": event_key,
            })),
        );
        match queue_res {
            Ok(Ok(_)) => Delivery::Sent,
            Ok(Err(_)) => {
                let current_idle = self
                    .state
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .threads
                    .get(&tid)
                    .is_some_and(|(s, _)| s.is_idle());
                if !current_idle {
                    return Delivery::NotReady;
                }
                match self.request(
                    "turn/start",
                    Some(json!({"threadId": &tid, "input": input})),
                ) {
                    Ok(Ok(_)) => Delivery::Sent,
                    Ok(Err(_)) | Err(_) => Delivery::Failed,
                }
            }
            Err(_) => Delivery::NotReady,
        }
    }
}

fn read_loop(mut reader: ws::WebSocketReader, state: Arc<Mutex<ClientState>>, gen_id: u64) {
    while let Ok(Some(text)) = reader.read_text() {
        let Ok(msg) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        if let Some(id) = msg.get("id").and_then(Value::as_u64)
            && msg.get("method").is_none()
        {
            let sender = state
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .pending
                .remove(&id);
            if let Some(tx) = sender {
                let _ = tx.send(msg);
            }
            continue;
        }
        match msg.get("method").and_then(Value::as_str) {
            Some("thread/started") => {
                if let Some(tid) = msg["params"]["thread"]["id"].as_str() {
                    let status = if msg["params"]["thread"]["status"].is_object() {
                        ThreadStatus::from_value(&msg["params"]["thread"]["status"])
                    } else {
                        ThreadStatus::Unknown
                    };
                    let mut guard = state.lock().unwrap_or_else(|p| p.into_inner());
                    if guard.generation == gen_id && !guard.threads.contains_key(tid) {
                        guard.thread_seq += 1;
                        let seq = guard.thread_seq;
                        guard.threads.insert(tid.to_owned(), (status, seq));
                    }
                }
            }
            Some("thread/status/changed") => {
                if let Some(tid) = msg["params"]["threadId"].as_str() {
                    let status = ThreadStatus::from_value(&msg["params"]["status"]);
                    let mut guard = state.lock().unwrap_or_else(|p| p.into_inner());
                    if guard.generation == gen_id {
                        guard.thread_seq += 1;
                        let seq = guard.thread_seq;
                        guard.threads.insert(tid.to_owned(), (status, seq));
                    }
                }
            }
            _ => {}
        }
    }
    let mut guard = state.lock().unwrap_or_else(|p| p.into_inner());
    if guard.generation == gen_id {
        guard.connected = false;
        guard.writer = None;
        guard.shutdown = None;
        guard.pending.clear();
        guard.threads.clear();
    }
}
