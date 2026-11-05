use super::*;

#[test]
fn native_prompts_keep_task_and_meeting_audits_in_their_threads() {
    let dir = tempfile::tempdir().unwrap();
    let mut task = snapshot("task", "succeeded");
    task["summary"]["mode"] = json!("task");
    task["summary"]["manager_session"] = json!(MANAGER);
    task["director"] = json!({"mode": "task", "tasks": [{
    "id": "one", "provider": "claude", "model": "m", "status": "succeeded",
    "prompt": "work", "output": "done", "time": "2026-10-15T00:00:00Z", "elapsed_ms": 9000,
    "prompts": [
        {"prompt_id": "allow", "invocation_id": "call", "kind": "permission", "tool_name": "Write", "tool_input": "{\"file_path\":\"/tmp/a\"}", "status": "answered", "created_at": "2026-10-15T00:00:01Z", "answer": {"actor": "boss", "decision": "allow", "answered_at": "2026-10-15T00:00:02Z"}},
        {"prompt_id": "deny", "kind": "permission", "tool_name": "Bash", "status": "answered", "created_at": "2026-10-15T00:00:01Z", "answer": {"actor": "boss", "decision": "deny", "message": "outside scope", "answered_at": "2026-10-15T00:00:03Z"}},
        {"prompt_id": "question", "kind": "question", "status": "answered", "created_at": "2026-10-15T00:00:01Z", "answer": {"actor": "boss", "answers": {"Where?": "Here"}, "answered_at": "2026-10-15T00:00:04Z"}},
        {"prompt_id": "cancel", "kind": "permission", "status": "answered", "created_at": "2026-10-15T00:00:01Z", "answer": {"actor": "service_cancel", "decision": "deny", "answered_at": "2026-10-15T00:00:05Z"}},
        {"prompt_id": "closed", "kind": "permission", "status": "closed", "created_at": "2026-10-15T00:00:01Z", "closed_at": "2026-10-15T00:00:06Z"}
    ]}]});
    let mut show = snapshot("meeting", "running");
    show["summary"]["manager_session"] = json!(MANAGER);
    show["director"] = json!({"mode": "show", "seats": [{"id": "a", "provider": "claude", "model": "m"}],
    "messages": [{"kind": "task", "speaker": "@host", "round": 1, "time": "2026-10-15T00:00:00Z", "text": "round one"}, {"kind": "task", "speaker": "@host", "round": 2, "time": "2026-10-15T00:00:07Z", "text": "round two"}],
    "rounds": [{"number": 1, "seats": [{"seat_id": "a", "prompts": task["director"]["tasks"][0]["prompts"]}]}],
    "round": {"number": 2, "seats": [{"seat_id": "a", "status": "running", "prompts": [
        {"prompt_id": "pending", "invocation_id": "next", "kind": "question", "questions": [{"question": "Where?"}], "status": "pending", "created_at": "2026-10-15T00:00:08Z"}
    ]}]}});
    save(dir.path(), &task);
    save(dir.path(), &show);
    let reader = Reader::new(dir.path().to_path_buf());
    let chat = state(&reader, &format!("/app/chats/{MANAGER}"));
    let messages = chat["messages"].as_array().unwrap();
    let events: Vec<_> = messages
        .iter()
        .filter(|m| m["kind"] == "approval")
        .collect();
    assert_eq!(events.len(), 11);
    let times: Vec<_> = events.iter().map(|m| m["time"].as_str().unwrap()).collect();
    assert!(times.windows(2).all(|times| times[0] <= times[1]));
    for event in &events {
        assert_eq!(event["visibility"], "private");
        assert_ne!(event["speaker"]["role"], "manager");
        if event["conversation"] == "task" {
            assert_eq!(event["thread"]["kind"], "desk");
            assert_eq!(event["reply_to"], "task:task:one:prompt");
        } else {
            assert_eq!(event["thread"], json!({"kind": "room", "id": "meeting"}));
            assert_eq!(event["desk"], "seat:meeting:a");
            assert_eq!(event["focus"], "seat:meeting:a");
            assert!(event["reply_to"].is_string());
        }
    }
    let allowed = events
        .iter()
        .find(|m| m["id"] == "task:task:one:approval:call:allow")
        .unwrap();
    assert_eq!(allowed["time"], "2026-10-15T00:00:02Z");
    assert_eq!(
        allowed["approval"]["tool_input"],
        "{\"file_path\":\"/tmp/a\"}"
    );
    assert_eq!(allowed["approval"]["answer"]["decision"], "allow");
    let pending = events
        .iter()
        .find(|m| m["approval"]["status"] == "pending")
        .unwrap();
    assert_eq!(pending["time"], "2026-10-15T00:00:08Z");
    assert_eq!(pending["approval"]["questions"][0]["question"], "Where?");
    // Resolve the same event, and include the latest round in both projections: no duplicate.
    let prompt = &mut show["director"]["round"]["seats"][0]["prompts"][0];
    prompt["status"] = json!("answered");
    prompt["answer"] = json!({"actor": "boss", "answers": {"Where?": "There"}, "answered_at": "2026-10-15T00:00:10Z"});
    let latest = show["director"]["round"].clone();
    show["director"]["rounds"]
        .as_array_mut()
        .unwrap()
        .push(latest);
    save(dir.path(), &show);
    let chat = state(&reader, &format!("/app/chats/{MANAGER}"));
    let updated: Vec<_> = chat["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["kind"] == "approval")
        .collect();
    assert_eq!(updated.len(), 11);
    let resolved = updated.iter().find(|m| m["id"] == pending["id"]).unwrap();
    assert_eq!(resolved["time"], "2026-10-15T00:00:10Z");
    assert_eq!(resolved["approval"]["answer"]["answers"]["Where?"], "There");
}
