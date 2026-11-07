use super::*;

#[test]
fn desktop_legacy_rules_and_cli_rules_keep_independent_modes_and_user_text() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path().canonicalize().unwrap();
    let apps = [
        App {
            key: "trae-desktop",
            id: "trae",
            label: "TRAE CN",
            root: home.join(".trae-cn"),
        },
        App {
            key: "trae",
            id: "trae",
            label: "TRAE CLI",
            root: home.join(".trae"),
        },
    ];
    let legacy = home.join(".trae-cn/user_rules.md");
    let multi = home.join(".trae-cn/user_rules/own.md");
    let cli = home.join(".trae/AGENTS.md");
    for path in [&legacy, &multi, &cli] {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "用户原文\n").unwrap();
    }
    let document = json!({"preset":"off", "apps":{"trae-desktop":"parallel", "trae":"split"}});
    assert_eq!(
        effective(&apps, &home, &document),
        json!({"trae-desktop":"parallel", "trae":"split"})
    );
    let backups = std::cell::RefCell::new(Vec::new());
    let backup = |path: &Path, _: &[u8]| {
        backups.borrow_mut().push(path.to_owned());
        Ok(())
    };
    assert!(sync(&apps, &apps, &home, &document, &[], &backup, false).is_empty());
    let desktop = std::fs::read_to_string(&legacy).unwrap();
    assert!(desktop.starts_with("用户原文\n"));
    assert!(desktop.contains(PARALLEL));
    let terminal = std::fs::read_to_string(&cli).unwrap();
    assert!(terminal.starts_with("用户原文\n"));
    assert!(!terminal.contains(PARALLEL));
    assert_eq!(std::fs::read_to_string(&multi).unwrap(), "用户原文\n");
    assert!(backups.borrow().contains(&legacy));
    assert!(!home.join(".trae-cn/user_rules/agentnave.md").exists());
    assert!(sync(&apps, &[], &home, &document, &[], &backup, false).is_empty());
    assert_eq!(std::fs::read_to_string(&legacy).unwrap(), "用户原文\n");
    assert_eq!(std::fs::read_to_string(&cli).unwrap(), "用户原文\n");
}

#[test]
fn concurrent_desktop_rule_edit_is_not_overwritten() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path().canonicalize().unwrap();
    let path = home.join(".trae-cn/user_rules.md");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "original").unwrap();
    let backup = |path: &Path, _: &[u8]| {
        std::fs::write(path, "concurrent").unwrap();
        Ok(())
    };
    assert!(
        write(&path, &home, Some("block"), &backup)
            .unwrap_err()
            .contains("其他程序")
    );
    assert_eq!(std::fs::read_to_string(path).unwrap(), "concurrent");
}
