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
    let expected = if cfg!(unix) {
        hosts.context.app.join("service/bin/agentnave-mcp")
    } else {
        new.clone()
    };
    assert!(old.is_file(), "old launcher stays valid during migration");
    for id in ["deepseek", "codex", "claude"] {
        let h = host(&hosts, id);
        let entry = h.read().unwrap().entry.unwrap();
        assert!(
            same_path(h.command(&entry).unwrap(), &expected),
            "{id}: {entry}"
        );
        assert_eq!(entry["keep"], "unchanged");
        assert_eq!(entry["args"], json!([]));
        assert_eq!(entry["env"]["AGENTNAVE_HOST"], h.boss);
    }
    #[cfg(unix)]
    {
        let configs: Vec<_> = ["deepseek", "codex", "claude"]
            .into_iter()
            .map(|id| {
                (
                    host(&hosts, id).config.clone(),
                    fs::read(host(&hosts, id).config).unwrap(),
                )
            })
            .collect();
        let next = install_next(&hosts, "0.59.0");
        crate::update::write_active_manifest(
            &hosts.context.app.join("service"),
            &json!({"version":"0.59.0","bin":next}),
        )
        .unwrap();
        hosts.refresh_probed();
        for (path, before) in configs {
            assert_eq!(fs::read(path).unwrap(), before);
        }
        assert!(
            fs::read_to_string(expected)
                .unwrap()
                .contains("0.59.0/bin/agentnave-mcp")
        );
    }
}

#[cfg(unix)]
fn install_next(hosts: &Hosts, version: &str) -> PathBuf {
    let root = hosts.context.app.join("service").join(version);
    let bin = root.join("bin/agentnave-mcp");
    executable_file(&bin, "#!/bin/sh\nexit 99\n");
    save(
        &root
            .join("tools/agentnave/Lib/site-packages")
            .join(format!("agentnave-{version}.dist-info/entry_points.txt")),
        "[console_scripts]\nagentnave-mcp = agentnave.relay:main\n",
    );
    bin
}

#[cfg(unix)]
#[test]
fn existing_hand_edited_launchers_are_not_repointed() {
    let (_temp, mut hosts) = fixture();
    let h = host(&hosts, "codex");
    executable_file(
        &hosts.context.search[0].join("codex"),
        "#!/bin/sh\nexit 99\n",
    );
    let document = Document::parse(&seed(&h), h.format, h.key).unwrap();
    for relative in [
        "custom/service/1.0.0/bin/agentnave-mcp",
        "custom/agentnave-mcp",
    ] {
        let command = hosts.context.home.join(relative);
        executable_file(&command, "#!/bin/sh\nexit 99\n");
        let entry = json!({"command":command,"args":[],"env":host_env(&h)});
        assert_eq!(hosts.command_state(&h, &entry), "present");
        save(&h.config, &document.render(Some(&entry)).unwrap());
        hosts.refresh_probed();
        assert_eq!(h.read().unwrap().entry.unwrap()["command"], json!(command));
    }
}
