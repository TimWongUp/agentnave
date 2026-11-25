use super::*;

#[test]
fn stopping_or_closing_a_boss_takes_its_derived_terminals_with_it() {
    for close in [false, true] {
        let office = Office::new();
        // After `/clear` the boss is in another workspace; what it dispatched before is still its.
        let (boss, boss_session) = office.boss(&[WS, "s-after-clear"]);
        let (other_boss, other_session) = office.boss(&["s-other"]);
        let (idle, idle_session) = office.derived("claude");
        let busy_session = cli();
        let busy = office.register("claude", Some("busy"), busy_session.clone(), Some("inv"));
        let (foreign, foreign_session) = office.derived("claude");
        office.with(foreign, |r| {
            r.derived.as_mut().unwrap().parent_workspace_id = Some("s-other".to_owned());
        });
        assert_eq!(office.row(idle)["parent_terminal_id"], boss);
        assert_eq!(office.row(foreign)["parent_terminal_id"], other_boss);
        let reply = if close {
            office.terminals.close(Some(boss))
        } else {
            office.terminals.stop(Some(boss))
        };
        assert_eq!(reply.status, 200);
        for session in [&boss_session, &idle_session, &busy_session] {
            ended(session);
        }
        assert!(office.row(idle).is_null() && office.row(busy).is_null());
        assert_eq!(office.row(boss).is_null(), close);
        assert!(foreign_session.exit().is_none());
        assert_eq!(office.state(foreign), "idle");
        foreign_session.stop();
        other_session.stop();
    }
}

#[test]
fn a_boss_that_exits_by_itself_leaves_what_is_in_use_until_it_is_idle() {
    let office = Office::new();
    let (boss, boss_session) = office.boss(&[WS]);
    let (idle, idle_session) = office.derived("claude");
    let (busy, busy_session) = office.derived("claude");
    office.with(busy, |r| r.native = Some("busy".to_owned()));
    report(&busy_session, "working");
    let (drafted, drafted_session) = office.derived("claude");
    drafted_session.user_input(b"draft", false).unwrap();
    let (collapsed, collapsed_session) = office.derived("claude");
    collapsed_session.stop();
    ended(&collapsed_session);
    office.beat(Duration::ZERO);
    assert_eq!(office.rows().len(), 5);
    assert_eq!(office.state(collapsed), "dormant");

    boss_session.stop();
    ended(&boss_session);
    office.beat(Duration::from_secs(1));
    ended(&idle_session);
    assert!(office.row(idle).is_null() && office.row(collapsed).is_null());
    assert_eq!(office.state(busy), "running");
    assert_eq!(office.state(drafted), "idle");
    assert_eq!(office.row(busy)["parent_terminal_id"], boss);

    // Done, it stays three more minutes and then goes with its row.
    let stop = json!({"hook_event_name": "Stop", "session_id": "busy", "background_tasks": []});
    office.hook(busy, stop);
    report(&busy_session, "done");
    let done = Duration::from_secs(10);
    office.beat(done);
    office.beat(done + IDLE_GRACE - Duration::from_secs(1));
    assert_eq!(office.state(busy), "idle");
    office.beat(done + IDLE_GRACE);
    ended(&busy_session);
    assert!(office.row(busy).is_null());
    assert!(drafted_session.exit().is_none());
    drafted_session.stop();
}

#[test]
fn a_boss_never_seen_here_neither_takes_nor_clears_anything() {
    let mut office = Office::new();
    let (_, boss_session) = office.boss(&["s-other"]);
    let (id, session) = office.derived("claude");
    boss_session.stop();
    ended(&boss_session);
    office.idle_for(IDLE_GRACE);
    ended(&session);
    office.beat(IDLE_GRACE * 2);
    assert_eq!(office.state(id), "dormant");
    assert!(office.row(id)["parent_terminal_id"].is_null());

    // Known by its chat alone, a boss is still remembered once its CLI is gone.
    let (keyed, keyed_session) = office.boss(&[]);
    office.chats.insert(keyed, vec![PARENT.to_owned()]);
    office.beat(IDLE_GRACE * 2);
    assert_eq!(office.row(id)["parent_terminal_id"], keyed);
    keyed_session.stop();
    ended(&keyed_session);
    office.chats.clear();
    office.beat(IDLE_GRACE * 2);
    assert!(office.row(id).is_null());
}

#[test]
fn close_derived_closes_a_workspaces_derived_terminals() {
    let office = Office::new();
    let (boss, boss_session) = office.boss(&[WS]);
    let (_, first) = office.derived("claude");
    let (_, second) = office.derived("antigravity");
    let (kept, kept_session) = office.derived("claude");
    office.with(kept, |r| {
        r.derived.as_mut().unwrap().parent_workspace_id = Some("s-other".to_owned());
    });
    let reply = office.terminals.close_derived(WS);
    assert_eq!(reply.status, 200);
    assert_eq!(
        serde_json::from_slice::<Value>(&reply.body).unwrap(),
        json!({"closed": 2})
    );
    ended(&first);
    ended(&second);
    let ids: Vec<_> = office.rows().iter().map(|r| r["id"].clone()).collect();
    assert_eq!(ids, [json!(boss), json!(kept)]);
    assert_eq!(office.terminals.close_derived("").status, 400);
    kept_session.stop();
    boss_session.stop();
}
