use super::*;

#[test]
fn existing_versioned_launchers_follow_active_manifest_across_host_formats() {
    let (_temp, mut hosts) = fixture();
    let install = |version: &str| {
        let root = hosts.context.app.join("service").join(version);
        let bin = root.join(if cfg!(windows) {
            "bin/agentnave-mcp.exe"
        } else {
            "bin/agentnave-mcp"
        });
        save(&bin, "launcher");
        save(
            &root
                .join("tools/agentnave/Lib/site-packages")
                .join(format!("agentnave-{version}.dist-info/entry_points.txt")),
            "[console_scripts]\nagentnave-mcp = agentnave.relay:main\n",
        );
        bin
    };
    let old = install("0.57.0");
    let new = install("0.58.0");
    for id in ["deepseek", "codex", "claude"] {
        let h = host(&hosts, id);
        executable_file(&hosts.context.search[0].join(h.binary), "cli");
        let document = Document::parse(&seed(&h), h.format, h.key).unwrap();
        let entry =
            json!({"command":old,"args":[],"env":{"AGENTNAVE_HOST":h.boss},"keep":"unchanged"});
        save(&h.config, &document.render(Some(&entry)).unwrap());
    }
    save(
        &hosts.context.app.join("service/active.json"),
        &json!({"version":"0.58.0","bin":new}).to_string(),
    );
    hosts.refresh_probed();
    assert!(old.is_file(), "old launcher stays valid during migration");
    for id in ["deepseek", "codex", "claude"] {
        let h = host(&hosts, id);
        let entry = h.read().unwrap().entry.unwrap();
        assert!(same_path(h.command(&entry).unwrap(), &new), "{id}: {entry}");
        assert_eq!(entry["keep"], "unchanged");
        assert_eq!(entry["args"], json!([]));
        assert_eq!(entry["env"]["AGENTNAVE_HOST"], h.boss);
    }
}
