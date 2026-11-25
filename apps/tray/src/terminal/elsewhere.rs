//! A chat already open outside the App's terminals (ADR-0103). Its CLI's relay is alive and not
//! under any terminal here, so the chat is running in some other program, or sits stopped there.
//! Only a CLI connected to AgentNave has a relay to be seen by; a derived session, which need not
//! have spoken to it, is also told by a CLI started on its native session.

use std::path::{Path, PathBuf};

use sysinfo::{Pid, ProcessRefreshKind, ProcessStatus, ProcessesToUpdate, System, UpdateKind};

use super::links;

/// How far up from a relay the program it runs in is looked for.
const DEPTH: usize = 64;

/// Where an open chat is: the application whose process tree holds it, when it has one, and
/// whether its processes are stopped (Ctrl+Z).
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Elsewhere {
    pub app: Option<String>,
    pub paused: bool,
}

/// One process in a relay's ancestry.
struct Link {
    exe: Option<PathBuf>,
    stopped: bool,
}

/// Where the chat of boss `key` is open, when it is not in one of the `terminals` (id, CLI pid).
pub(super) fn find(root: &Path, key: &str, terminals: &[(u64, u32)]) -> Option<Elsewhere> {
    if !sysinfo::IS_SUPPORTED_SYSTEM {
        return None;
    }
    let relays: Vec<links::Relay> = links::relays(&root.join("connections"))
        .into_iter()
        .filter(|relay| relay.bosses.iter().any(|boss| boss == key))
        .collect();
    if relays.is_empty() {
        return None;
    }
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_exe(UpdateKind::OnlyIfNotSet),
    );
    relays
        .into_iter()
        // A relay file whose process is gone or whose PID was reused is a crash leftover, not an open chat.
        .filter(|relay| {
            system
                .process(Pid::from_u32(relay.pid))
                .is_some_and(|process| links::same_process(process, relay.started_ms))
        })
        .filter(|relay| links::terminal_of(&system, relay.pid, terminals).is_none())
        .map(|relay| describe(&ancestry(&system, relay.pid)))
        // Prefer a running copy over a stopped one when the chat is open in several places.
        .min_by_key(|found| found.paused)
}

/// Where a process started on native session `native` runs, when not in one of the `terminals`.
pub(super) fn started_on(native: &str, terminals: &[(u64, u32)]) -> Option<Elsewhere> {
    if native.is_empty() || !sysinfo::IS_SUPPORTED_SYSTEM {
        return None;
    }
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing()
            .with_exe(UpdateKind::OnlyIfNotSet)
            .with_cmd(UpdateKind::OnlyIfNotSet),
    );
    system
        .processes()
        .values()
        .filter(|process| process.cmd().iter().any(|arg| names(arg, native)))
        .map(|process| process.pid().as_u32())
        .filter(|pid| links::terminal_of(&system, *pid, terminals).is_none())
        .map(|pid| describe(&ancestry(&system, pid)))
        .min_by_key(|found| found.paused)
}

/// Whether an argument is the session itself, alone or as an option's value.
fn names(arg: &std::ffi::OsStr, native: &str) -> bool {
    let arg = arg.to_string_lossy();
    arg.strip_suffix(native)
        .is_some_and(|before| before.is_empty() || before.ends_with('='))
}

fn ancestry(system: &System, relay: u32) -> Vec<Link> {
    let mut chain = Vec::new();
    let mut current = system.process(Pid::from_u32(relay));
    while let Some(process) = current
        && chain.len() < DEPTH
    {
        chain.push(Link {
            exe: process.exe().map(Path::to_owned),
            stopped: process.status() == ProcessStatus::Stop,
        });
        current = process.parent().and_then(|parent| system.process(parent));
    }
    chain
}

/// The first application in the ancestry names the place; the chat is paused when anything
/// between it and the relay is stopped.
fn describe(chain: &[Link]) -> Elsewhere {
    let app = chain.iter().position(|link| app_name(link).is_some());
    let reach = app.map_or(chain.len(), |at| at + 1);
    Elsewhere {
        app: app.and_then(|at| app_name(&chain[at])),
        paused: chain[..reach].iter().any(|link| link.stopped),
    }
}

/// The name of the `.app` bundle an executable lives in.
fn app_name(link: &Link) -> Option<String> {
    link.exe
        .as_deref()?
        .ancestors()
        .find(|dir| dir.extension().is_some_and(|ext| ext == "app"))
        .and_then(Path::file_stem)
        .map(|name| name.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(exe: &str, stopped: bool) -> Link {
        Link {
            exe: Some(PathBuf::from(exe)),
            stopped,
        }
    }

    #[test]
    fn the_first_application_up_the_tree_names_the_place() {
        let chain = [
            link("/opt/bin/agentnave-mcp", false),
            link("/Users/x/.local/bin/claude", false),
            link("/bin/zsh", false),
            link("/Applications/Ghostty.app/Contents/MacOS/ghostty", false),
            link("/sbin/launchd", false),
        ];
        assert_eq!(
            describe(&chain),
            Elsewhere {
                app: Some("Ghostty".to_owned()),
                paused: false
            }
        );
    }

    #[test]
    fn a_stopped_process_below_the_application_pauses_the_chat() {
        let chain = [
            link("/opt/bin/agentnave-mcp", false),
            link("/Users/x/.local/bin/claude", true),
            link(
                "/Applications/Utilities/Terminal.app/Contents/MacOS/Terminal",
                false,
            ),
        ];
        let found = describe(&chain);
        assert_eq!(found.app.as_deref(), Some("Terminal"));
        assert!(found.paused);
        // A stopped process above the application is no concern of this chat.
        let above = [
            link("/Applications/Ghostty.app/Contents/MacOS/ghostty", false),
            link("/sbin/launchd", true),
        ];
        assert!(!describe(&above).paused);
    }

    #[cfg(unix)]
    #[test]
    fn a_cli_started_on_the_session_is_found_unless_a_terminal_here_holds_it() {
        let native = "5d0c6b0e-5f0c-4a59-9f3d-elsewhere";
        let spawn = |argument: &str| {
            std::process::Command::new("/bin/sh")
                .args(["-c", "read line", "sh", argument])
                .stdin(std::process::Stdio::piped())
                .spawn()
                .unwrap()
        };
        assert_eq!(started_on(native, &[]), None);
        for argument in [native.to_owned(), format!("--resume={native}")] {
            let mut cli = spawn(&argument);
            assert!(started_on(native, &[]).is_some(), "{argument}");
            assert_eq!(started_on(native, &[(1, cli.id())]), None);
            cli.kill().unwrap();
            cli.wait().unwrap();
        }
        let mut other = spawn(&format!("about {native}"));
        assert_eq!(started_on(native, &[]), None);
        other.kill().unwrap();
        other.wait().unwrap();
    }

    #[test]
    fn a_chat_in_no_application_has_no_name() {
        let chain = [
            link("/usr/bin/sshd", false),
            Link {
                exe: None,
                stopped: false,
            },
        ];
        assert_eq!(
            describe(&chain),
            Elsewhere {
                app: None,
                paused: false
            }
        );
    }
}
