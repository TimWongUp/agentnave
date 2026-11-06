use super::*;
use std::process::{Command, Stdio};

#[test]
fn fixed_script_executes_versioned_target_and_preserves_arguments() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp
        .path()
        .join("Application Support/it's AgentNave/service");
    let target = base.join("1.2.0/bin/agentnave-mcp");
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    fs::write(&target, "#!/bin/sh\nprintf '%s\\n' \"$$\" \"$@\"\n").unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).unwrap();
    write(&base, &target).unwrap();
    let fixed = base.join("bin/agentnave-mcp");
    assert_eq!(
        fs::metadata(&fixed).unwrap().permissions().mode() & 0o777,
        0o755
    );
    let child = Command::new(&fixed)
        .args(["two words", "'quoted'", "$(literal)"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let pid = child.id().to_string();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        [pid.as_str(), "two words", "'quoted'", "$(literal)"]
    );
    let new = base.join("1.3.0/bin/agentnave-mcp");
    write(&base, &new).unwrap();
    assert!(
        fs::read_to_string(&fixed)
            .unwrap()
            .contains("1.3.0/bin/agentnave-mcp")
    );
    assert_eq!(fs::read_dir(fixed.parent().unwrap()).unwrap().count(), 1);
}

#[test]
fn failed_manifest_swap_restores_the_previous_fixed_script() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path();
    write(base, Path::new("/old/bin/agentnave-mcp")).unwrap();
    let before = fs::read(base.join("bin/agentnave-mcp")).unwrap();
    fs::create_dir(base.join("active.json")).unwrap();
    let result = crate::update::write_active_manifest(
        base,
        &serde_json::json!({"version":"new","bin":"/new/bin/agentnave-mcp"}),
    );
    assert!(result.is_err());
    assert_eq!(fs::read(base.join("bin/agentnave-mcp")).unwrap(), before);
    assert_eq!(fs::read_dir(base).unwrap().count(), 2);
}
