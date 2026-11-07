use super::*;
use crate::detect::{DetectionCache, detected_with};

#[test]
fn independent_updates_run_together_and_duplicate_provider_is_refused() {
    let temp = TempDir::new().unwrap();
    let a = file(temp.path(), ".codex/packages/standalone/1/codex");
    let b = file(temp.path(), ".grok/bin/grok");
    for (path, peer) in [(&a, &b), (&b, &a)] {
        fs::write(path.with_extension("version"), "1.0.0\n").unwrap();
        fs::write(
            path,
            format!(
                r#"#!/bin/sh
if [ "$1" != update ]; then cat "$0.version"; exit; fi
: > "$0.started"
n=0
while [ ! -f '{}.started' ]; do
  n=$((n+1)); [ "$n" -lt 1000 ] || {{ echo "peer update did not start" >&2; exit 8; }}
  sleep 0.01
done
sleep 0.1
echo 2.0.0 > "$0.version"
"#,
                peer.display()
            ),
        )
        .unwrap();
    }
    let locks = locks::Updates::default();
    let cache = Mutex::new(DetectionCache {
        all: Some(json!({
            "codex": {"path": a, "version": "1.0.0"},
            "grok": {"path": b, "version": "1.0.0"},
        })),
        ..Default::default()
    });
    std::thread::scope(|scope| {
        let first = scope.spawn(|| {
            update_at(
                "codex",
                &a,
                false,
                &locks,
                &cache,
                |_| Some("2.0.0".into()),
                Duration::from_secs(20),
            )
        });
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !a.with_extension("started").exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            a.with_extension("started").exists(),
            "first command started"
        );
        assert!(
            update_at(
                "codex",
                &a,
                false,
                &locks,
                &cache,
                |_| panic!("duplicate must not query"),
                Duration::from_secs(20)
            )
            .unwrap_err()
            .contains("已有 CLI")
        );
        let second = scope.spawn(|| {
            update_at(
                "grok",
                &b,
                false,
                &locks,
                &cache,
                |_| Some("2.0.0".into()),
                Duration::from_secs(20),
            )
        });
        assert_eq!(first.join().unwrap().unwrap(), "2.0.0");
        assert_eq!(second.join().unwrap().unwrap(), "2.0.0");
    });
    let all = detected_with(false, &cache, |_| panic!("must use cache"), |_| {});
    assert_eq!(all["codex"]["version"], "2.0.0");
    assert_eq!(all["grok"]["version"], "2.0.0");
}

#[test]
fn shared_npm_environment_serializes_commands_and_version_verification() {
    let temp = TempDir::new().unwrap();
    let npm = file(temp.path(), "node/bin/npm");
    fs::write(
        &npm,
        r#"#!/bin/sh
prefix="$4"
mkdir "$prefix/busy" || exit 8
case "$5" in
  @openai/*) cli=codex;;
  *) cli=claude;;
esac
sleep 0.05
echo 2.0.0 > "$prefix/$cli.version"
rmdir "$prefix/busy"
"#,
    )
    .unwrap();
    let prefix = npm.parent().unwrap().parent().unwrap();
    let mut paths = Vec::new();
    for (provider, package) in [
        ("codex", "@openai/codex"),
        ("claude", "@anthropic-ai/claude-code"),
    ] {
        let path = file(
            prefix,
            &format!("lib/node_modules/{package}/bin/{provider}"),
        );
        fs::write(prefix.join(format!("{provider}.version")), "1.0.0\n").unwrap();
        fs::write(
            &path,
            format!(
                r#"#!/bin/sh
prefix='{}'
mkdir "$prefix/busy" || exit 8
cat "$prefix/{provider}.version"
sleep 0.05
rmdir "$prefix/busy"
"#,
                prefix.display()
            ),
        )
        .unwrap();
        paths.push((provider, path));
    }
    let locks = locks::Updates::default();
    let cache = Mutex::new(DetectionCache::default());
    let barrier = std::sync::Barrier::new(2);
    std::thread::scope(|scope| {
        let jobs: Vec<_> = paths
            .iter()
            .map(|(provider, path)| {
                let barrier = &barrier;
                let locks = &locks;
                let cache = &cache;
                scope.spawn(move || {
                    barrier.wait();
                    update_at(
                        provider,
                        path,
                        false,
                        locks,
                        cache,
                        |_| Some("2.0.0".into()),
                        Duration::from_secs(20),
                    )
                })
            })
            .collect();
        for job in jobs {
            assert_eq!(job.join().unwrap().unwrap(), "2.0.0");
        }
    });
}

#[test]
fn environment_keys_follow_npm_prefix_and_brew_not_package() {
    let temp = TempDir::new().unwrap();
    file(temp.path(), "brew/bin/brew");
    let mut keys = Vec::new();
    for (provider, name) in [("codex", "codex"), ("claude", "claude-code")] {
        let path = file(temp.path(), &format!("brew/Cellar/{name}/1/bin/{provider}"));
        keys.push(install::identify(provider, &path).unwrap().environment);
    }
    assert_eq!(keys[0], keys[1]);
    file(temp.path(), "other-brew/bin/brew");
    let other = file(temp.path(), "other-brew/Cellar/codex/1/bin/codex");
    assert_ne!(
        keys[0],
        install::identify("codex", &other).unwrap().environment
    );
    let guard = locks::Updates::default();
    assert!(guard.claim(true, "codex").is_err());
}

#[test]
fn late_full_probe_cannot_undo_a_verified_update() {
    // Cover both a forced refresh and the first, still-unpublished detection.
    for seeded in [true, false] {
        let temp = TempDir::new().unwrap();
        let path = file(temp.path(), ".grok/bin/grok");
        fs::write(path.with_extension("version"), "1.0.0\n").unwrap();
        fs::write(
            &path,
            r#"#!/bin/sh
if [ "$1" = update ]; then echo 2.0.0 > "$0.version"; else cat "$0.version"; fi
"#,
        )
        .unwrap();
        let info = || {
            json!({"installed": true, "path": path,
            "version": read_version(&path), "models": [], "error": null})
        };
        let cache = Mutex::new(DetectionCache {
            all: seeded.then(|| json!({"grok": info()})),
            ..Default::default()
        });
        let locks = locks::Updates::default();
        let (taken, captured) = std::sync::mpsc::channel();
        let (release, blocked) = std::sync::mpsc::channel();
        let blocked = Mutex::new(blocked);
        std::thread::scope(|scope| {
            let refresh = scope.spawn(|| {
                detected_with(
                    seeded,
                    &cache,
                    |provider| match provider {
                        "grok" => {
                            let old = info();
                            taken.send(()).unwrap();
                            old
                        }
                        "claude" => {
                            blocked
                                .lock()
                                .unwrap()
                                .recv_timeout(Duration::from_secs(10))
                                .unwrap();
                            json!({"installed": false})
                        }
                        _ => json!({"installed": false}),
                    },
                    |_| {},
                )
            });
            captured.recv_timeout(Duration::from_secs(10)).unwrap();
            let updated = update_at(
                "grok",
                &path,
                false,
                &locks,
                &cache,
                |_| Some("2.0.0".into()),
                Duration::from_secs(5),
            );
            // Always release the other probe before asserting, even if update fails.
            release.send(()).unwrap();
            assert_eq!(updated.unwrap(), "2.0.0");
            let result = refresh.join().unwrap();
            assert_eq!(result["grok"]["version"], "2.0.0");
            assert_eq!(
                detected_with(
                    false,
                    &cache,
                    |_| panic!("cached reads must not probe"),
                    |_| {}
                )["grok"]["version"],
                "2.0.0"
            );
        });
        // A later refresh must observe an external downgrade, not pin the verified update.
        fs::write(path.with_extension("version"), "0.9.0\n").unwrap();
        let refreshed = detected_with(
            true,
            &cache,
            |provider| {
                if provider == "grok" {
                    info()
                } else {
                    json!({"installed": false})
                }
            },
            |_| {},
        );
        assert_eq!(refreshed["grok"]["version"], "0.9.0");
    }
}
