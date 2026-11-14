use super::*;

#[test]
fn switches_main_skill_and_removes_both_on_disconnect() {
    let (_temp, mut hosts) = fixture();
    let h = host(&hosts, "pi");
    executable_file(&hosts.context.search[0].join("pi"), "#!/bin/sh\nexit 99\n");
    hosts.action("connect", &json!({"id":"pi"})).unwrap();
    let manager = h.skill.as_ref().unwrap();
    let meeting = h.optional("agentnave-meeting").unwrap();
    assert_eq!(
        skill::state(manager, &hosts.context.home, skill::MANAGER)["state"],
        "app"
    );
    crate::members::save_orchestration(
        &hosts.context.data,
        &json!({"version":1,"preset":"meetings","effective_apps":{"pi":"meetings"}}),
    )
    .unwrap();
    hosts.ensure_skill(&h, true).unwrap();
    assert_eq!(
        skill::state(&meeting, &hosts.context.home, skill::MEETING)["state"],
        "app"
    );
    assert_eq!(
        skill::state(manager, &hosts.context.home, skill::MANAGER)["state"],
        "missing"
    );
    assert_eq!(hosts.state(&h)["skill"]["state"], "app");
    // A damaged orchestration file switches nothing: the meeting Skill stays.
    let file = hosts.context.data.join("orchestration.json");
    let saved = std::fs::read(&file).unwrap();
    std::fs::write(&file, b"{").unwrap();
    hosts.ensure_skill(&h, true).unwrap();
    assert_eq!(
        skill::state(&meeting, &hosts.context.home, skill::MEETING)["state"],
        "app"
    );
    assert_eq!(
        skill::state(manager, &hosts.context.home, skill::MANAGER)["state"],
        "missing"
    );
    assert_eq!(hosts.state(&h)["skill"]["state"], "app");
    std::fs::write(&file, saved).unwrap();
    crate::members::save_orchestration(
        &hosts.context.data,
        &json!({"version":1,"preset":"off","effective_apps":{"pi":"off"}}),
    )
    .unwrap();
    hosts.ensure_skill(&h, true).unwrap();
    assert_eq!(
        skill::state(manager, &hosts.context.home, skill::MANAGER)["state"],
        "app"
    );
    assert_eq!(
        skill::state(&meeting, &hosts.context.home, skill::MEETING)["state"],
        "missing"
    );
    skill::install(
        &meeting,
        &hosts.context.app,
        "pi",
        &hosts.context.home,
        skill::MEETING,
    )
    .unwrap();
    hosts.action("remove", &json!({"id":"pi"})).unwrap();
    assert!(!manager.exists() && !meeting.exists());
}

#[test]
fn shared_directory_prefers_manager_for_a_connected_non_meeting_row() {
    let (temp, mut hosts) = fixture();
    let shared = temp.path().canonicalize().unwrap().join("buddy");
    for key in ["CODEBUDDY_CONFIG_DIR", "WORKBUDDY_CONFIG_DIR"] {
        hosts
            .context
            .env
            .insert(key.into(), shared.display().to_string());
    }
    for id in ["codebuddy", "workbuddy"] {
        let h = host(&hosts, id);
        executable_file(
            &hosts.context.search[0].join(h.binary),
            "#!/bin/sh\nexit 99\n",
        );
        hosts.action("connect", &json!({"id":id})).unwrap();
    }
    let h = host(&hosts, "codebuddy");
    crate::members::save_orchestration(&hosts.context.data, &json!({"version":1,"preset":"meetings","effective_apps":{"codebuddy":"meetings","workbuddy":"split"}})).unwrap();
    hosts.ensure_skill(&h, true).unwrap();
    assert_eq!(
        skill::state(
            h.skill.as_ref().unwrap(),
            &hosts.context.home,
            skill::MANAGER
        )["state"],
        "app"
    );
    assert!(!h.optional("agentnave-meeting").unwrap().exists());
}
