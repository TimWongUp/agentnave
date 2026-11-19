//! A chat already open outside the App's terminals (ADR-0103). Its CLI's relay is alive and not
//! under any terminal here, so the chat is running in some other program, or sits stopped there.
//! Only a CLI connected to AgentNave has a relay to be seen by.

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
    let relays: Vec<u32> = links::relays(&root.join("connections"))
        .into_iter()
        .filter(|(_, bosses)| bosses.iter().any(|boss| boss == key))
        .map(|(pid, _)| pid)
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
        // A relay file whose process is gone is a crash leftover, not an open chat.
        .filter(|relay| system.process(Pid::from_u32(*relay)).is_some())
        .filter(|relay| links::terminal_of(&system, *relay, terminals).is_none())
        .map(|relay| describe(&ancestry(&system, relay)))
        // Prefer a running copy over a stopped one when the chat is open in several places.
        .min_by_key(|found| found.paused)
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
