//! Read-only local checks enabled explicitly with `--features windows-live-tests`.
use super::*;

#[test]
fn windows_live_snapshot() {
    let context = Context::live(
        dirs::data_dir()
            .unwrap()
            .join("io.github.timwongup.agentnave"),
    );
    let (shell, dirs) = probe_tools(&context);
    let hosts = Hosts::from_probe(context, Some(shell), Arc::new(Mutex::new(Some(dirs))));
    let snapshot = hosts.snapshot();
    println!("snapshot at: {}", timestamp());
    println!("service: {}", snapshot["service"]);
    assert_eq!(snapshot["service"]["version"], VERSION);
    let mut required = Vec::new();
    for row in snapshot["hosts"].as_array().unwrap() {
        println!(
            "{}",
            json!({"id":row["id"], "status":row["status"], "reload":row["reload"], "launcher":row["launcher"]})
        );
        let id = row["id"].as_str().unwrap();
        println!(
            "reload records {id}: {}",
            json!({
                "observed":hosts.settings[id]["reload_observed"],
                "changed":hosts.settings[id]["reload_changed"],
                "confirmed":hosts.settings[id]["reload_confirmed"]
            })
        );
        if row["reload"]["state"] == "required" {
            required.push(id);
        }
    }
    assert_eq!(snapshot["hosts"].as_array().unwrap().len(), 10);
    assert!(
        required.is_empty(),
        "hosts still requiring reload: {required:?}"
    );
}
