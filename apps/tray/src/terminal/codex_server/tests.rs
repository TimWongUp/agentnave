use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use base64::Engine;
use serde_json::{Value, json};

use super::{Client, Delivery, ThreadStatus, launch_args, ws};
use crate::data::boss_app::boss_key;

pub(in crate::terminal) fn server_handshake(stream: &mut UnixStream) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut key = String::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let trimmed = line.trim();
        if trimmed.is_empty() {
            break;
        }
        if let Some((name, val)) = trimmed.split_once(':')
            && name.trim().eq_ignore_ascii_case("sec-websocket-key")
        {
            key = val.trim().to_owned();
        }
    }
    let accept = base64::engine::general_purpose::STANDARD.encode(ws::sha1(&format!(
        "{key}258EAFA5-E914-47DA-95CA-C5AB0DC85B11"
    )));
    let response = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
         Sec-WebSocket-Accept: {accept}\r\n\r\n"
    );
    stream.write_all(response.as_bytes()).unwrap();
    stream.flush().unwrap();
}

pub(in crate::terminal) fn read_client_frame(stream: &mut UnixStream) -> Option<String> {
    let mut head = [0u8; 2];
    if stream.read_exact(&mut head).is_err() {
        return None;
    }
    let opcode = head[0] & 0x0f;
    let masked = head[1] & 0x80 != 0;
    let len = match head[1] & 0x7f {
        n @ 0..=125 => usize::from(n),
        126 => {
            let mut b = [0u8; 2];
            stream.read_exact(&mut b).ok()?;
            usize::from(u16::from_be_bytes(b))
        }
        127 => {
            let mut b = [0u8; 8];
            stream.read_exact(&mut b).ok()?;
            usize::try_from(u64::from_be_bytes(b)).ok()?
        }
        _ => unreachable!(),
    };
    let mut mask = [0u8; 4];
    if masked {
        stream.read_exact(&mut mask).ok()?;
    }
    let mut payload = vec![0u8; len];
    stream.read_exact(&mut payload).ok()?;
    if masked {
        for (i, b) in payload.iter_mut().enumerate() {
            *b ^= mask[i % 4];
        }
    }
    if opcode == 0x8 {
        return None;
    }
    Some(String::from_utf8(payload).unwrap())
}

pub(in crate::terminal) fn write_server_frame(stream: &mut UnixStream, text: &str) {
    let payload = text.as_bytes();
    let mut out = vec![0x81u8];
    if payload.len() < 126 {
        out.push(payload.len() as u8);
    } else {
        out.push(126);
        out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    }
    out.extend_from_slice(payload);
    let _ = stream.write_all(&out);
    let _ = stream.flush();
}

pub(in crate::terminal) fn connect_to_mock(
    sock: &Path,
    handler: impl Fn(&Value, &mut UnixStream) + Send + Sync + 'static,
) -> Arc<Client> {
    let listener = UnixListener::bind(sock).unwrap();
    let handler = Arc::new(handler);
    thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            let handler = handler.clone();
            thread::spawn(move || {
                server_handshake(&mut stream);
                while let Some(text) = read_client_frame(&mut stream) {
                    let msg: Value = serde_json::from_str(&text).unwrap();
                    handler(&msg, &mut stream);
                }
            });
        }
    });
    let client = Arc::new(Client {
        sock: sock.to_owned(),
        state: Arc::default(),
    });
    client.try_connect().unwrap();
    client
}

#[test]
fn wrapper_launch_args_cover_new_and_resume_without_permission_overrides() {
    let (program, args) = launch_args(
        Path::new("/opt/homebrew/bin/codex"),
        Path::new("/tmp/an-cx-1-2.sock"),
        Path::new("/tmp/project"),
        None,
        &[],
    );
    assert_eq!(program, Path::new("/bin/sh"));
    assert_eq!(args[0], "-c");
    assert_eq!(args[2], "agentnave-codex");
    assert_eq!(args[3], "/opt/homebrew/bin/codex");
    assert_eq!(args[4], "/tmp/an-cx-1-2.sock");
    assert_eq!(args[5], "/tmp/project");
    assert_eq!(args[6], "");
    assert!(args[1].contains("check_for_update_on_startup=false"));
    assert!(!args[1].contains("approval_policy"));
    assert!(!args[1].contains("sandbox_mode"));

    let (_, resume_args) = launch_args(
        Path::new("/opt/homebrew/bin/codex"),
        Path::new("/tmp/an-cx-1-2.sock"),
        Path::new("/tmp/project"),
        Some("thread-abc"),
        &["-c".into(), "model=gpt-6-luna".into()],
    );
    assert_eq!(resume_args[6], "thread-abc");
    assert_eq!(resume_args[7..], ["-c", "model=gpt-6-luna"]);
}

#[test]
fn delivers_via_queue_add_and_skips_waiting_on_approval() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("mock.sock");
    let tid = "019a0000-0000-7000-8000-000000000001";
    let key = boss_key("codex", tid);
    let recorded: Arc<Mutex<Vec<Value>>> = Arc::default();
    let rec = recorded.clone();
    let client = connect_to_mock(&sock, move |msg, stream| {
        rec.lock().unwrap().push(msg.clone());
        let id = msg.get("id").and_then(Value::as_u64);
        match msg.get("method").and_then(Value::as_str) {
            Some("initialize") => {
                assert_eq!(
                    msg["params"]["capabilities"]["experimentalApi"],
                    json!(true)
                );
                write_server_frame(
                    stream,
                    &json!({"jsonrpc": "2.0", "id": id, "result": {}}).to_string(),
                );
            }
            Some("thread/loaded/list") => {
                write_server_frame(
                    stream,
                    &json!({"jsonrpc": "2.0", "id": id, "result": {"data": [tid]}}).to_string(),
                );
            }
            Some("thread/read") => {
                write_server_frame(
                    stream,
                    &json!({"jsonrpc": "2.0", "id": id, "result": {"thread": {"id": tid, "status": {"type": "active", "activeFlags": ["waitingOnApproval"]}}}}).to_string(),
                );
            }
            Some("thread/queue/add") => {
                write_server_frame(
                    stream,
                    &json!({"jsonrpc": "2.0", "id": id, "result": {"queuedSubmission": {"id": "q1", "clientUserMessageId": msg["params"]["clientUserMessageId"], "input": []}}}).to_string(),
                );
            }
            _ => {}
        }
    });

    assert_eq!(client.deliver(&key, "evt-1", "hi"), Delivery::NotReady);
    client.state.lock().unwrap().threads.insert(
        tid.to_owned(),
        (
            ThreadStatus::Active {
                waiting_on_approval: false,
            },
            99,
        ),
    );
    assert_eq!(
        client.deliver(&key, "evt-1", "hello"),
        Delivery::Queued {
            thread_id: tid.to_owned(),
            submission_id: "q1".to_owned(),
        }
    );

    let sent = recorded
        .lock()
        .unwrap()
        .iter()
        .find(|m| m["method"] == "thread/queue/add")
        .cloned()
        .unwrap();
    assert_eq!(sent["params"]["threadId"], tid);
    assert_eq!(sent["params"]["clientUserMessageId"], "evt-1");
    assert_eq!(sent["params"]["input"][0]["text"], "hello");
}

#[test]
fn falls_back_to_turn_start_only_when_idle() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("fallback.sock");
    let tid = "019a0000-0000-7000-8000-000000000002";
    let key = boss_key("codex", tid);
    let methods: Arc<Mutex<Vec<String>>> = Arc::default();
    let recorded = methods.clone();
    let client = connect_to_mock(&sock, move |msg, stream| {
        if let Some(m) = msg.get("method").and_then(Value::as_str) {
            recorded.lock().unwrap().push(m.to_owned());
        }
        let id = msg.get("id").and_then(Value::as_u64);
        match msg.get("method").and_then(Value::as_str) {
            Some("initialize") => write_server_frame(
                stream,
                &json!({"jsonrpc": "2.0", "id": id, "result": {}}).to_string(),
            ),
            Some("thread/loaded/list") => write_server_frame(
                stream,
                &json!({"jsonrpc": "2.0", "id": id, "result": {"data": [tid]}}).to_string(),
            ),
            Some("thread/read") => write_server_frame(
                stream,
                &json!({"jsonrpc": "2.0", "id": id, "result": {"thread": {"id": tid, "status": {"type": "active", "activeFlags": []}}}}).to_string(),
            ),
            Some("thread/queue/add") => write_server_frame(
                stream,
                &json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "unsupported"}}).to_string(),
            ),
            Some("turn/start") => {
                assert!(msg["params"].get("approvalPolicy").is_none());
                assert!(msg["params"].get("sandboxPolicy").is_none());
                write_server_frame(
                    stream,
                    &json!({"jsonrpc": "2.0", "id": id, "result": {"turn": {"id": "t1"}}}).to_string(),
                );
            }
            _ => {}
        }
    });

    // Busy thread must not fall back to turn/start (would steer active turn).
    assert_eq!(client.deliver(&key, "evt-2", "msg"), Delivery::NotReady);
    assert!(!methods.lock().unwrap().iter().any(|m| m == "turn/start"));

    // Idle thread falls back to turn/start.
    client
        .state
        .lock()
        .unwrap()
        .threads
        .insert(tid.to_owned(), (ThreadStatus::Idle, 99));
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(2)
        && client.deliver(&key, "evt-2", "msg") != Delivery::Started
    {
        thread::sleep(Duration::from_millis(20));
    }
    assert!(methods.lock().unwrap().iter().any(|m| m == "turn/start"));
}

#[test]
fn refresh_overwrites_stale_status_removes_unloaded_and_handles_unknown_read_failure() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("refresh.sock");
    let tid = "019a0000-0000-7000-8000-000000000003";
    let gone_tid = "019a0000-0000-7000-8000-000000000004";
    let key = boss_key("codex", tid);
    let phase = Arc::new(AtomicUsize::new(0));
    let p = phase.clone();
    let turn_calls = Arc::new(AtomicUsize::new(0));
    let tc = turn_calls.clone();

    let client = connect_to_mock(&sock, move |msg, stream| {
        let id = msg.get("id").and_then(Value::as_u64);
        match msg.get("method").and_then(Value::as_str) {
            Some("initialize") => write_server_frame(
                stream,
                &json!({"jsonrpc": "2.0", "id": id, "result": {}}).to_string(),
            ),
            Some("thread/loaded/list") => {
                let list = if p.load(Ordering::SeqCst) == 0 {
                    json!([tid, gone_tid])
                } else {
                    json!([tid])
                };
                write_server_frame(
                    stream,
                    &json!({"jsonrpc": "2.0", "id": id, "result": {"data": list}}).to_string(),
                );
            }
            Some("thread/read") => {
                let target = msg["params"]["threadId"].as_str().unwrap_or_default();
                let current_phase = p.load(Ordering::SeqCst);
                if target == gone_tid || current_phase == 0 {
                    write_server_frame(
                        stream,
                        &json!({"jsonrpc": "2.0", "id": id, "result": {"thread": {"id": target, "status": {"type": "idle"}}}}).to_string(),
                    );
                } else if current_phase == 1 {
                    // Overwrites previously cached Idle with waitingOnApproval.
                    write_server_frame(
                        stream,
                        &json!({"jsonrpc": "2.0", "id": id, "result": {"thread": {"id": target, "status": {"type": "active", "activeFlags": ["waitingOnApproval"]}}}}).to_string(),
                    );
                } else {
                    // thread/read error -> Unknown: queue/add still tried, turn/start NOT tried.
                    write_server_frame(
                        stream,
                        &json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32000, "message": "no rollout found"}}).to_string(),
                    );
                }
            }
            Some("thread/queue/add") => {
                if p.load(Ordering::SeqCst) == 3 {
                    write_server_frame(
                        stream,
                        &json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "unsupported"}}).to_string(),
                    );
                } else {
                    write_server_frame(
                        stream,
                        &json!({"jsonrpc": "2.0", "id": id, "result": {"queuedSubmission": {"id": "q", "clientUserMessageId": "k", "input": []}}}).to_string(),
                    );
                }
            }
            Some("turn/start") => {
                tc.fetch_add(1, Ordering::SeqCst);
                write_server_frame(
                    stream,
                    &json!({"jsonrpc": "2.0", "id": id, "result": {"turn": {"id": "t"}}})
                        .to_string(),
                );
            }
            _ => {}
        }
    });

    assert_eq!(client.thread_status_for_test(tid), Some(ThreadStatus::Idle));
    assert_eq!(
        client.thread_status_for_test(gone_tid),
        Some(ThreadStatus::Idle)
    );

    // Phase 1: refresh removes gone_tid and overwrites tid to waitingOnApproval.
    phase.store(1, Ordering::SeqCst);
    client.refresh_threads();
    assert_eq!(client.thread_status_for_test(gone_tid), None);
    assert_eq!(
        client.thread_status_for_test(tid),
        Some(ThreadStatus::Active {
            waiting_on_approval: true
        })
    );
    assert_eq!(client.deliver(&key, "e1", "hi"), Delivery::NotReady);

    // Phase 2: thread/read fails -> Unknown allows queue/add.
    phase.store(2, Ordering::SeqCst);
    client.refresh_threads();
    assert_eq!(
        client.thread_status_for_test(tid),
        Some(ThreadStatus::Unknown)
    );
    assert_eq!(
        client.deliver(&key, "e2", "hi"),
        Delivery::Queued {
            thread_id: tid.to_owned(),
            submission_id: "q".to_owned(),
        }
    );

    // Phase 3: Unknown + queue/add error does NOT fall back to turn/start.
    phase.store(3, Ordering::SeqCst);
    assert_eq!(client.deliver(&key, "e3", "hi"), Delivery::NotReady);
    assert_eq!(turn_calls.load(Ordering::SeqCst), 0);

    // Disconnect clears thread cache.
    let gen_id = client.state.lock().unwrap().generation;
    client.disconnect_generation(gen_id);
    assert!(client.state.lock().unwrap().threads.is_empty());
}

#[test]
fn stale_generation_disconnect_does_not_clear_newer_connection() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("gen.sock");
    let tid = "019a0000-0000-7000-8000-000000000005";
    let client = connect_to_mock(&sock, move |msg, stream| {
        let id = msg.get("id").and_then(Value::as_u64);
        match msg.get("method").and_then(Value::as_str) {
            Some("initialize") => write_server_frame(
                stream,
                &json!({"jsonrpc": "2.0", "id": id, "result": {}}).to_string(),
            ),
            Some("thread/loaded/list") => write_server_frame(
                stream,
                &json!({"jsonrpc": "2.0", "id": id, "result": {"data": [tid]}}).to_string(),
            ),
            Some("thread/read") => write_server_frame(
                stream,
                &json!({"jsonrpc": "2.0", "id": id, "result": {"thread": {"id": tid, "status": {"type": "idle"}}}}).to_string(),
            ),
            _ => {}
        }
    });

    let first_gen = client.state.lock().unwrap().generation;
    client.try_connect().unwrap();
    let second_gen = client.state.lock().unwrap().generation;
    assert!(second_gen > first_gen);

    // Disconnecting the old generation must leave the newer generation intact.
    client.disconnect_generation(first_gen);
    assert!(client.is_connected());
    assert_eq!(client.thread_status_for_test(tid), Some(ThreadStatus::Idle));
}

#[test]
fn close_unblocks_stalled_handshake_promptly() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("stalled.sock");
    let listener = UnixListener::bind(&sock).unwrap();
    let server = thread::spawn(move || {
        let (_stream, _) = listener.accept().unwrap();
        thread::sleep(Duration::from_secs(3));
    });

    let client = Arc::new(Client {
        sock: sock.clone(),
        state: Arc::default(),
    });
    let closer = client.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(80));
        closer.close();
    });

    let started = Instant::now();
    let result = client.try_connect();
    assert!(result.is_err());
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "close() should unblock stalled handshake immediately"
    );
    let _ = server.join();
}

#[test]
fn deliver_if_fresh_skips_stale_and_queue_delete_update_send_expected_params() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("queue-ops.sock");
    let tid = "019a0000-0000-7000-8000-000000000006";
    let key = boss_key("codex", tid);
    let recorded: Arc<Mutex<Vec<Value>>> = Arc::default();
    let rec = recorded.clone();
    let client = connect_to_mock(&sock, move |msg, stream| {
        rec.lock().unwrap().push(msg.clone());
        let id = msg.get("id").and_then(Value::as_u64);
        match msg.get("method").and_then(Value::as_str) {
            Some("initialize") => write_server_frame(
                stream,
                &json!({"jsonrpc": "2.0", "id": id, "result": {}}).to_string(),
            ),
            Some("thread/loaded/list") => write_server_frame(
                stream,
                &json!({"jsonrpc": "2.0", "id": id, "result": {"data": [tid]}}).to_string(),
            ),
            Some("thread/read") => write_server_frame(
                stream,
                &json!({"jsonrpc": "2.0", "id": id, "result": {"thread": {"id": tid, "status": {"type": "active", "activeFlags": []}}}}).to_string(),
            ),
            Some("thread/queue/update") => write_server_frame(
                stream,
                &json!({"jsonrpc": "2.0", "id": id, "result": {"queuedSubmission": {"id": "q9", "clientUserMessageId": "k", "input": []}}}).to_string(),
            ),
            Some("thread/queue/delete") => write_server_frame(
                stream,
                &json!({"jsonrpc": "2.0", "id": id, "result": {"deleted": true}}).to_string(),
            ),
            _ => {}
        }
    });

    assert_eq!(client.deliver_if_fresh(&key, || None), Delivery::NotReady);
    assert!(
        !recorded
            .lock()
            .unwrap()
            .iter()
            .any(|m| m["method"] == "thread/queue/add")
    );
    assert!(client.update_queued(tid, "q9", "updated"));
    assert!(client.delete_queued(tid, "q9"));

    let msgs = recorded.lock().unwrap();
    let update = msgs
        .iter()
        .find(|m| m["method"] == "thread/queue/update")
        .unwrap();
    assert_eq!(update["params"]["threadId"], tid);
    assert_eq!(update["params"]["queuedSubmissionId"], "q9");
    assert_eq!(update["params"]["input"][0]["text"], "updated");
    let delete = msgs
        .iter()
        .find(|m| m["method"] == "thread/queue/delete")
        .unwrap();
    assert_eq!(delete["params"]["threadId"], tid);
    assert_eq!(delete["params"]["queuedSubmissionId"], "q9");
}
