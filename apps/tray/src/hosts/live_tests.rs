//! Opt-in checks against the validation machine. Normal cargo test never touches real configs.
use super::*;

#[test]
#[ignore = "reads real host configurations and process/connection observations"]
fn windows_live_snapshot() {
    let context = Context::live(
        dirs::data_dir()
            .unwrap()
            .join("io.github.timwongup.agentnave"),
    );
    let (shell, dirs) = probe_tools(&context);
    let hosts = Hosts::from_probe(context, Some(shell), Arc::new(Mutex::new(Some(dirs))));
    let snapshot = hosts.snapshot();
    println!("service: {}", snapshot["service"]);
    assert_eq!(snapshot["service"]["version"], "0.58.0");
    for row in snapshot["hosts"].as_array().unwrap() {
        println!(
            "{}",
            json!({"id":row["id"], "status":row["status"], "reload":row["reload"], "launcher":row["launcher"]})
        );
    }
}

#[test]
#[ignore = "explicitly repoints real managed host entries to the active launcher"]
fn windows_live_refresh() {
    assert_eq!(std::env::var("AGENTNAVE_LIVE_REFRESH").as_deref(), Ok("1"));
    let context = Context::live(
        dirs::data_dir()
            .unwrap()
            .join("io.github.timwongup.agentnave"),
    );
    let (shell, dirs) = probe_tools(&context);
    let mut hosts = Hosts::from_probe(context, Some(shell), Arc::new(Mutex::new(Some(dirs))));
    let snapshot = hosts.refresh_probed();
    for id in ["deepseek", "codex", "claude"] {
        let row = snapshot["hosts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == id)
            .unwrap();
        println!(
            "{}",
            json!({"id":row["id"],"launcher":row["launcher"],"attention":row["attention"]})
        );
        assert_eq!(row["launcher"]["stale"], false);
    }
}

#[test]
#[ignore = "switches real host launchers from an installed 0.57.0 baseline to 0.58.0"]
fn windows_live_upgrade() {
    assert_eq!(std::env::var("AGENTNAVE_LIVE_REFRESH").as_deref(), Ok("1"));
    let app = dirs::data_dir()
        .unwrap()
        .join("io.github.timwongup.agentnave");
    let pointer = app.join("service/active.json");
    let original = fs::read(&pointer).unwrap();
    struct Restore(PathBuf, Vec<u8>);
    impl Drop for Restore {
        fn drop(&mut self) {
            fs::write(&self.0, &self.1).unwrap();
        }
    }
    let restore = Restore(pointer.clone(), original.clone());
    let old = app.join("service/0.57.0/bin/agentnave-mcp.exe");
    assert!(old.is_file());
    fs::write(&pointer, json!({"version":"0.57.0", "bin":old}).to_string()).unwrap();
    let context = Context::live(app.clone());
    let (shell, dirs) = probe_tools(&context);
    let mut hosts = Hosts::from_probe(context, Some(shell), Arc::new(Mutex::new(Some(dirs))));
    hosts.refresh_probed();
    for id in ["deepseek", "codex", "claude"] {
        let h = hosts.registry().into_iter().find(|h| h.id == id).unwrap();
        let entry = h.read().unwrap().entry.unwrap();
        println!("before {id}: {}", h.command(&entry).unwrap());
        assert!(same_path(h.command(&entry).unwrap(), &old));
    }
    fs::write(&pointer, original).unwrap();
    let snapshot = hosts.refresh_probed();
    for id in ["deepseek", "codex", "claude"] {
        let row = snapshot["hosts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == id)
            .unwrap();
        println!("after {id}: {}", row["launcher"]);
        assert_eq!(row["launcher"]["stale"], false);
    }
    drop(restore);
}
