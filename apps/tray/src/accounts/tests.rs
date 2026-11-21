use super::request::{Context, request};
use super::*;
use serde_json::json;
use std::sync::Mutex;

fn account(id: &str, dir: &Path, managed: bool) -> Account {
    Account {
        id: id.to_owned(),
        label: id.to_uppercase(),
        dir: dir.to_owned(),
        managed,
    }
}

/// A POST to an accounts route with no host connections behind it.
fn post(root: &Path, home: &Path, path: &str, body: Value) -> (u16, Value) {
    let hosts = Mutex::new(None);
    let idle = || Ok(());
    let ctx = Context {
        root,
        home: home.to_owned(),
        hosts: &hosts,
        idle: &idle,
    };
    let reply = request(
        &ctx,
        None,
        "POST",
        path,
        Some("1"),
        body.to_string().as_bytes(),
    );
    (
        reply.status,
        serde_json::from_slice(&reply.body).unwrap_or_default(),
    )
}

#[test]
fn labels_are_short_trimmed_names_without_control_characters() {
    assert_eq!(clean_label("  工作 ").unwrap(), "工作");
    assert!(clean_label("   ").is_err());
    assert!(clean_label(&"长".repeat(41)).is_err());
    assert!(clean_label("a\u{7}b").is_err());
    assert!(clean_label(&"长".repeat(40)).is_ok());
}

#[test]
fn a_missing_file_is_no_account_and_a_broken_one_says_so() {
    let root = tempfile::tempdir().unwrap();
    assert!(read(root.path()).unwrap().is_empty());
    fs::write(
        root.path().join(FILE),
        r#"{"version": 1, "accounts": [{"id": "A b"}]}"#,
    )
    .unwrap();
    assert!(read(root.path()).unwrap_err().contains("格式错误"));
    assert_eq!(state(root.path())["accounts"], json!([]));
    assert!(state(root.path())["error"].is_string());
}

#[test]
fn creating_makes_the_folder_and_ids_are_not_given_again() {
    let (root, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let first = create(root.path(), home.path(), "工作").unwrap();
    assert_eq!(first.id, "a1");
    assert!(first.managed && first.dir.is_dir());
    assert!(first.dir.ends_with(".agentnave-accounts/claude/a1"));
    let second = create(root.path(), home.path(), "个人").unwrap();
    assert_eq!(second.id, "a2");

    // The highest id is gone, folder and all; the next account still takes a new one.
    unbind(root.path(), "a2").unwrap();
    fs::remove_dir_all(&second.dir).unwrap();
    assert_eq!(create(root.path(), home.path(), "新").unwrap().id, "a3");
    let ids: Vec<_> = read(root.path())
        .unwrap()
        .into_iter()
        .map(|a| a.id)
        .collect();
    assert_eq!(ids, ["a1", "a3"]);
    // The saved file passes the shared check.
    let saved: Value = serde_json::from_slice(&fs::read(root.path().join(FILE)).unwrap()).unwrap();
    validate(&saved).unwrap();
}

#[test]
fn renaming_changes_only_the_label() {
    let (root, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let made = create(root.path(), home.path(), "工作").unwrap();
    rename(root.path(), "a1", " 公司 ").unwrap();
    let now = read(root.path()).unwrap();
    assert_eq!((now[0].id.as_str(), now[0].label.as_str()), ("a1", "公司"));
    assert_eq!(now[0].dir, made.dir);
    assert!(rename(root.path(), "a1", "").is_err());
    assert!(rename(root.path(), "zz", "x").is_err());
}

#[test]
fn only_the_folder_the_app_made_for_a_managed_account_is_deleted() {
    let (root, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let made = create(root.path(), home.path(), "工作").unwrap();
    fs::write(made.dir.join("chat.jsonl"), "x").unwrap();
    // An external folder, or a managed id pointed elsewhere, stays.
    let outside = home.path().join("elsewhere");
    fs::create_dir_all(&outside).unwrap();
    assert!(remove::deletable(&account("a9", &outside, false), home.path()).is_err());
    assert!(remove::deletable(&account("a1", &outside, true), home.path()).is_err());
    remove::delete(&made, home.path()).unwrap();
    assert!(!made.dir.exists() && outside.is_dir());
}

#[cfg(unix)]
#[test]
fn a_managed_folder_that_is_a_link_is_not_followed() {
    let (root, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let made = create(root.path(), home.path(), "工作").unwrap();
    let precious = home.path().join("precious");
    fs::create_dir_all(&precious).unwrap();
    fs::write(precious.join("keep"), "x").unwrap();
    fs::remove_dir_all(&made.dir).unwrap();
    std::os::unix::fs::symlink(&precious, &made.dir).unwrap();
    assert!(remove::delete(&made, home.path()).is_err());
    assert!(precious.join("keep").is_file());
}

#[test]
fn the_page_creates_renames_and_removes_accounts() {
    let (root, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (status, created) = post(
        root.path(),
        home.path(),
        "/app/accounts/create",
        json!({"label": "工作"}),
    );
    assert_eq!(status, 200, "{created}");
    assert_eq!(created["account"]["id"], "a1");
    assert_eq!(created["state"]["accounts"][0]["label"], "工作");
    let dir = PathBuf::from(created["account"]["dir"].as_str().unwrap());
    assert!(dir.is_dir());

    let blank = post(
        root.path(),
        home.path(),
        "/app/accounts/create",
        json!({"label": " "}),
    );
    assert_eq!(blank.0, 400);
    let (_, renamed) = post(
        root.path(),
        home.path(),
        "/app/accounts/rename",
        json!({"id": "a1", "label": "公司"}),
    );
    assert_eq!(renamed["state"]["accounts"][0]["label"], "公司");

    // Removing only unbinds: the folder and its sign-in stay.
    let remove = |body| post(root.path(), home.path(), "/app/accounts/remove", body);
    let (status, removed) = remove(json!({"id": "a1"}));
    assert_eq!(status, 200);
    assert_eq!(removed["state"]["accounts"], json!([]));
    assert_eq!(removed["deleted"], false);
    assert!(dir.is_dir());
    assert_eq!(remove(json!({"id": "a1"})).0, 404);
}

#[test]
fn deleting_the_folder_waits_for_running_work() {
    let (root, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let create = json!({"label": "工作"});
    let (_, created) = post(root.path(), home.path(), "/app/accounts/create", create);
    let dir = PathBuf::from(created["account"]["dir"].as_str().unwrap());
    let hosts = Mutex::new(None);
    let busy = || Err::<(), _>("还有任务在跑".to_owned());
    let ctx = Context {
        root: root.path(),
        home: home.path().to_owned(),
        hosts: &hosts,
        idle: &busy,
    };
    let body = json!({"id": "a1", "delete_dir": true}).to_string();
    let path = "/app/accounts/remove";
    let reply = request(&ctx, None, "POST", path, Some("1"), body.as_bytes());
    assert_eq!(reply.status, 409);
    // Nothing changed: still registered, folder still there.
    assert_eq!(read(root.path()).unwrap().len(), 1);
    assert!(dir.is_dir());
    let (status, removed) = post(
        root.path(),
        home.path(),
        path,
        json!({"id": "a1", "delete_dir": true}),
    );
    assert_eq!(status, 200);
    assert_eq!(removed["deleted"], true);
    assert!(!dir.exists());
}

#[test]
fn existing_external_accounts_stay_readable_renamable_and_unbind_without_deleting_their_folder() {
    let (root, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let folder = home.path().join(".claude_personal");
    fs::create_dir_all(folder.join("projects")).unwrap();
    let doc = json!({
        "version": 1,
        "accounts": [{
            "id": "a1",
            "provider": "claude",
            "label": "个人",
            "dir": folder.display().to_string(),
            "managed": false
        }]
    });
    fs::write(root.path().join(FILE), doc.to_string()).unwrap();

    let (_, renamed) = post(
        root.path(),
        home.path(),
        "/app/accounts/rename",
        json!({"id": "a1", "label": "外部个人"}),
    );
    assert_eq!(renamed["state"]["accounts"][0]["label"], "外部个人");
    assert_eq!(renamed["state"]["accounts"][0]["managed"], false);

    // Unbinding an existing external folder never deletes it, whatever the page asks.
    let (status, _) = post(
        root.path(),
        home.path(),
        "/app/accounts/remove",
        json!({"id": "a1", "delete_dir": true}),
    );
    assert_eq!(status, 409);
    let (status, removed) = post(
        root.path(),
        home.path(),
        "/app/accounts/remove",
        json!({"id": "a1", "delete_dir": false}),
    );
    assert_eq!(status, 200);
    assert_eq!(removed["state"]["accounts"], json!([]));
    assert!(folder.is_dir());

    // Retired link and scan routes return 404.
    assert_eq!(
        post(root.path(), home.path(), "/app/accounts/link", json!({})).0,
        404
    );
    let hosts = Mutex::new(None);
    let idle = || Ok(());
    let ctx = Context {
        root: root.path(),
        home: home.path().to_owned(),
        hosts: &hosts,
        idle: &idle,
    };
    assert_eq!(
        request(&ctx, None, "GET", "/app/accounts/scan", None, &[]).status,
        404
    );
}

#[cfg(unix)]
#[test]
fn the_sign_in_state_keeps_only_what_the_row_shows_and_reads_the_named_folder() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let cli = temp.path().join("claude");
    fs::write(
        &cli,
        "#!/bin/sh\nif [ \"$CLAUDE_CONFIG_DIR\" = \"/named\" ]; then\n\
         printf '{\"loggedIn\":true,\"authMethod\":\"claude.ai\",\"email\":\"a@b.c\",\
         \"subscriptionType\":\"max\",\"orgId\":\"secret\",\"orgName\":\"Org\"}'\n\
         else printf '{\"loggedIn\":false,\"authMethod\":\"none\"}'; exit 1; fi\n",
    )
    .unwrap();
    fs::set_permissions(&cli, fs::Permissions::from_mode(0o700)).unwrap();
    let named = auth::status_with(&cli, Some(Path::new("/named")));
    assert_eq!(
        named,
        json!({"logged_in": true, "method": "claude.ai", "email": "a@b.c",
            "plan": "max", "error": null})
    );
    let signed_out = auth::status_with(&cli, None);
    assert_eq!(signed_out["logged_in"], false);
    assert_eq!(signed_out["method"], "none");
    assert!(auth::status_with(&temp.path().join("missing"), None)["error"].is_string());
}
