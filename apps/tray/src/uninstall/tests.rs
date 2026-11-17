use super::*;
use std::path::{Path, PathBuf};

#[test]
fn preview_and_live_or_unknown_work_refuse_removal() {
    let mut overview = Overview::default();
    assert!(guard(true, &overview).is_err());
    assert!(guard(false, &overview).is_ok());
    overview.running = 1;
    assert!(guard(false, &overview).is_err());
    overview.running = 0;
    overview.review = 1;
    assert!(guard(false, &overview).is_err());
    overview.review = 0;
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(crate::db::path(temp.path()), b"invalid database").unwrap();
    let unreadable = Reader::new(temp.path().to_path_buf()).overview();
    assert!(guard(false, &unreadable).is_err());
}

#[test]
fn the_waiter_distinguishes_confirm_cancel_and_connection_loss() {
    for decision in [Some(true), Some(false), None] {
        let waiter = channel::Waiter::new().unwrap();
        let argument = waiter.argument();
        let worker = std::thread::spawn(move || {
            let reply = channel::Reply::connect(&argument).unwrap();
            if let Some(decision) = decision {
                reply.finish(decision);
            }
        });
        let (mut stream, pid) = waiter.accept().unwrap();
        assert_eq!(pid, std::process::id());
        match decision {
            Some(decision) => assert_eq!(channel::outcome(&mut stream).unwrap(), decision),
            None => assert!(channel::outcome(&mut stream).is_err()),
        }
        worker.join().unwrap();
    }
    assert!(channel::Reply::connect("12345:bad-token").is_err());
}

#[test]
fn deletion_targets_only_app_directories_and_explicitly_selected_history() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let app = home.join("app");
    let prefs = home.join("preferences");
    let history = home.join(".agentnave");
    let exe = temp.path().join("program/tray");
    for selected in [false, true] {
        let plan = cleanup::Plan::new(
            vec![app.clone(), prefs.clone(), app.clone()],
            &history,
            selected,
            &home,
            &exe,
        )
        .unwrap();
        let mut expected = vec![app.clone(), prefs.clone()];
        if selected {
            expected.push(history.clone());
        }
        expected.sort();
        assert_eq!(plan.directories, expected);
    }
    for bad in [&home, temp.path(), Path::new("/"), Path::new("relative")] {
        assert!(cleanup::Plan::new(vec![bad.to_path_buf()], &history, false, &home, &exe).is_err());
    }
    assert!(
        cleanup::Plan::new(vec![app.clone()], &app.join("records"), false, &home, &exe).is_err()
    );
    #[cfg(unix)]
    {
        let alias = temp.path().join("home-alias");
        std::fs::create_dir_all(&home).unwrap();
        std::os::unix::fs::symlink(&home, &alias).unwrap();
        assert!(cleanup::Plan::new(vec![alias], &history, false, &home, &exe).is_err());
        // HOME itself may be a link; the directory holding the link is still its ancestor.
        let links = temp.path().join("links");
        std::fs::create_dir_all(&links).unwrap();
        let linked_home = links.join("home");
        std::os::unix::fs::symlink(&home, &linked_home).unwrap();
        assert!(cleanup::Plan::new(vec![links], &history, false, &linked_home, &exe).is_err());
    }
    assert_eq!(
        cleanup::app_bundle(Path::new("/custom/AgentNave.app/Contents/MacOS/tray")),
        Some(PathBuf::from("/custom/AgentNave.app"))
    );
    assert_eq!(cleanup::app_bundle(&exe), None);
}

#[cfg(unix)]
#[test]
fn detached_cleanup_preserves_history_and_does_not_follow_links_or_expand_paths() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let app = home.join("app ' $(touch unexpected)");
    let history = home.join(".agentnave");
    std::fs::create_dir_all(&app).unwrap();
    std::fs::create_dir_all(&history).unwrap();
    let saved = history.join("records");
    std::fs::write(&saved, b"keep").unwrap();
    std::os::unix::fs::symlink(&history, app.join("linked-history")).unwrap();
    let plan = cleanup::Plan::new(
        vec![app.clone()],
        &history,
        false,
        &home,
        &temp.path().join("tray"),
    )
    .unwrap();
    // A reaped child gives the production wait loop a parent that has actually exited.
    let mut child = std::process::Command::new("/bin/sh")
        .args(["-c", "exit 0"])
        .spawn()
        .unwrap();
    let pid = child.id();
    child.wait().unwrap();
    assert!(cleanup::command(&plan, pid).status().unwrap().success());
    assert!(!app.exists());
    assert_eq!(std::fs::read(saved).unwrap(), b"keep");
    assert!(!temp.path().join("unexpected").exists());
}
