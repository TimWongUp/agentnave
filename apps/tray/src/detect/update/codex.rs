//! Two source categories only; discovering the secondary CLI never probes its models.
use super::*;
use std::collections::HashMap;
use std::time::Instant;

type VersionEntry = (Instant, Option<String>);
static VERSIONS: LazyLock<Mutex<HashMap<PathBuf, VersionEntry>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
const VERSION_TTL: Duration = Duration::from_secs(30);

pub(super) fn remember(path: &Path, version: Option<String>) {
    VERSIONS
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(path.to_owned(), (Instant::now(), version));
}

fn version(path: &Path) -> Option<String> {
    // Coalesce concurrent GETs too; a stale in-flight read cannot overwrite remember() after
    // an update has verified its new version. Only the secondary source needs this probe.
    let mut versions = VERSIONS.lock().unwrap_or_else(|p| p.into_inner());
    if let Some((at, version)) = versions.get(path)
        && at.elapsed() < VERSION_TTL
    {
        return version.clone();
    }
    let version = read_version(path);
    versions.insert(path.to_owned(), (Instant::now(), version.clone()));
    version
}

pub(super) fn paths(info: &Value) -> [Option<PathBuf>; 2] {
    let mut paths = [
        super::super::locate::codex_desktop(),
        super::super::locate::standalone_codex(),
    ];
    if let Some(active) = info["path"].as_str().map(Path::new) {
        let index = usize::from(!install::desktop("codex", active));
        paths[index] = Some(active.to_owned());
    }
    paths
}

pub(super) fn state(
    info: &Value,
    paths: &[Option<PathBuf>; 2],
    query: &impl Fn(&Channel) -> Option<String>,
) -> Value {
    let mut state = cli_state("codex", &Value::Null, |_| None);
    let mut installations = Vec::new();
    for (index, path) in paths
        .iter()
        .enumerate()
        .filter_map(|(i, p)| Some((i, p.as_ref()?)))
    {
        let active = info["path"]
            .as_str()
            .is_some_and(|p| same_path(Path::new(p), path));
        let version = if active {
            info["version"].clone()
        } else {
            json!(version(path))
        };
        let installation = json!({"installed": true, "path": path, "version": version});
        let mut entry = cli_state("codex", &installation, query);
        if active {
            state = entry.clone();
        }
        entry["id"] = json!(if index == 0 { "desktop" } else { "standalone" });
        entry["label"] = json!(if index == 0 {
            match super::super::locate::codex_desktop_app(path) {
                Some("ChatGPT") => "ChatGPT 内置 CLI",
                _ => "Codex 内置 CLI",
            }
        } else {
            "Codex CLI"
        });
        entry["path"] = json!(path);
        entry["active"] = json!(active);
        installations.push(entry);
    }
    state["installations"] = json!(installations);
    state
}
