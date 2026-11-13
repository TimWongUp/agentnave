use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use super::auto::HostDirs;
use super::classify::classify;
use super::env::Env;
use super::memory::Memory;
use super::path::{align_with, canonical, key, plain_windows, too_broad, within};
use super::{OTHER, Project};

/// A machine laid out under one directory: the platform places are injected so the fixture
/// itself, which sits in the real temporary directory, is an ordinary place.
struct Machine {
    _root: tempfile::TempDir,
    base: PathBuf,
    env: Env,
    /// The records live in the fixture too, never in the app's own config directory.
    records: PathBuf,
    memory: RefCell<Memory>,
}

impl Machine {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let base = canonical(root.path()).unwrap();
        let home = base.join("home");
        for dir in ["Desktop", "Downloads", "Documents", "Code"] {
            fs::create_dir_all(home.join(dir)).unwrap();
        }
        let dirs = HostDirs {
            home: home.clone(),
            documents: home.join("Documents"),
            claude: vec![home.join("Support/Claude")],
            codex_workspace: home.join("Documents/Codex"),
            zcode_base: home.join(".zcode"),
            workbuddy_workspace: home.join("WorkBuddy"),
        };
        let env = Env::new(
            dirs,
            vec![base.join("systmp")],
            home.join(".agentnave"),
            vec![home.join("Desktop"), home.join("Downloads")],
        );
        let records = base.join("config/projects.json");
        Self {
            _root: root,
            base,
            env,
            memory: RefCell::new(Memory::at(records.clone())),
            records,
        }
    }

    fn home(&self) -> PathBuf {
        self.base.join("home")
    }

    fn make(&self, relative: &str) -> PathBuf {
        let path = self.base.join(relative);
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn write(&self, relative: &str, text: &str) {
        let path = self.base.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn project(&self, cwd: impl AsRef<Path>) -> Project {
        classify(
            &cwd.as_ref().to_string_lossy(),
            &self.env,
            &mut self.memory.borrow_mut(),
        )
    }

    /// `self` as a start of the app would see it: the file is read again.
    fn restart(&self) {
        *self.memory.borrow_mut() = Memory::at(self.records.clone());
    }

    /// What a fresh reader finds in the records file.
    fn recorded(&self) -> BTreeMap<String, String> {
        serde_json::from_slice(&fs::read(&self.records).unwrap_or_default()).unwrap_or_default()
    }

    fn forget(&self, origins: &[PathBuf]) {
        let origins = origins
            .iter()
            .map(|cwd| cwd.to_string_lossy().into_owned())
            .collect::<BTreeSet<_>>();
        self.memory.borrow_mut().retain(&origins);
    }

    /// The key as a path below the fixture, or `OTHER`.
    fn place(&self, cwd: impl AsRef<Path>) -> String {
        let key = self.project(cwd).key;
        match Path::new(&key).strip_prefix(&self.base) {
            Ok(rest) => rest.to_string_lossy().into_owned(),
            Err(_) => key,
        }
    }
}

#[test]
fn unusable_directories_are_other() {
    let machine = Machine::new();
    for cwd in ["", "  ", "relative/dir", "../up"] {
        assert_eq!(machine.project(cwd).key, OTHER, "{cwd:?}");
    }
    assert_eq!(machine.project("").label, "");
    // A Linux or WSL path is not a path on Windows, and is not mapped there.
    if cfg!(windows) {
        for cwd in ["/mnt/c/Users/a/app", "/home/a/app", r"\Users\a\app"] {
            assert_eq!(machine.project(cwd).key, OTHER, "{cwd}");
        }
    }
}

#[test]
fn scratch_places_beat_repositories() {
    let machine = Machine::new();
    machine.make("systmp/repo/.git");
    machine.make("home/.agentnave/work/.git");
    assert_eq!(machine.place(machine.base.join("systmp/repo/src")), OTHER);
    assert_eq!(machine.place(machine.base.join("systmp")), OTHER);
    assert_eq!(machine.place(machine.home().join(".agentnave/work")), OTHER);
    // Names that only start like the temporary directory are projects.
    assert_eq!(machine.place(machine.base.join("systmp2/x")), "systmp2/x");
    assert_eq!(
        machine.place(machine.base.join("SystmpReports")),
        "SystmpReports"
    );
}

#[test]
fn repositories_and_their_subdirectories_are_one_project() {
    let machine = Machine::new();
    machine.make("home/Code/app/.git");
    machine.make("home/Code/app/src");
    let app = "home/Code/app";
    for cwd in [
        "home/Code/app",
        "home/Code/app/src",
        // Never created, or removed since: an agent worktree inside the repository.
        "home/Code/app/.worktrees/gone",
        "home/Code/app/../app/src/./deep/er",
    ] {
        assert_eq!(machine.place(machine.base.join(cwd)), app, "{cwd}");
    }
    let project = machine.project(machine.base.join("home/Code/app/src"));
    assert_eq!(project.label, "app");
    assert_eq!(project.path, "~/Code/app");
}

#[test]
fn linked_worktrees_belong_to_the_main_checkout() {
    let machine = Machine::new();
    machine.make("home/Code/app/.git/worktrees/wt");
    machine.write("home/Code/app/.git/worktrees/wt/commondir", "../..\n");
    machine.write("home/Code/wt/.git", "gitdir: ../app/.git/worktrees/wt\n");
    machine.make("home/Code/wt/src");
    assert_eq!(
        machine.place(machine.base.join("home/Code/wt/src")),
        "home/Code/app"
    );
    // The same pointer written with backslashes and an absolute path.
    machine.write(
        "home/Code/wt2/.git",
        &format!(
            "gitdir: {}\n",
            machine
                .base
                .join("home/Code/app/.git/worktrees/wt")
                .to_string_lossy()
                .replace('/', "\\")
        ),
    );
    if cfg!(windows) {
        assert_eq!(
            machine.place(machine.base.join("home/Code/wt2")),
            "home/Code/app"
        );
    }
}

#[test]
fn submodules_and_broken_pointers_stop_at_their_own_directory() {
    let machine = Machine::new();
    machine.make("home/Code/app/.git/modules/sub");
    machine.write("home/Code/app/sub/.git", "gitdir: ../.git/modules/sub\n");
    machine.make("home/Code/app/.git");
    assert_eq!(
        machine.place(machine.base.join("home/Code/app/sub")),
        "home/Code/app/sub"
    );
    machine.write("home/Code/bad/.git", "not a pointer");
    machine.write("home/Code/missing/.git", "gitdir: ../nowhere\n");
    machine.write(
        "home/Code/long/.git",
        &format!("gitdir: {}", "a".repeat(5000)),
    );
    machine.write("home/Code/empty/.git", "gitdir:\n");
    for name in ["bad", "missing", "long", "empty"] {
        let cwd = machine.base.join("home/Code").join(name).join("x");
        assert_eq!(machine.place(cwd), format!("home/Code/{name}"));
    }
    // A commondir that does not name a `.git` directory is not followed.
    machine.make("home/Code/odd/gitdirs/a");
    machine.write("home/Code/odd/gitdirs/a/commondir", "..\n");
    machine.write("home/Code/oddwt/.git", "gitdir: ../odd/gitdirs/a\n");
    assert_eq!(
        machine.place(machine.base.join("home/Code/oddwt")),
        "home/Code/oddwt"
    );
}

#[test]
fn outside_repositories_a_directory_is_its_own_project_even_when_deleted() {
    let machine = Machine::new();
    machine.make("home/Code/plain");
    for cwd in [
        "home/Code/plain",
        "home/Code/lost/deep",
        "home/Code/plainer",
    ] {
        assert_eq!(machine.place(machine.base.join(cwd)), cwd);
    }
    assert_eq!(
        machine.project(machine.base.join("home/Code/lost")).label,
        "lost"
    );
}

#[test]
fn the_home_desktop_and_downloads_are_not_projects_but_what_is_inside_is() {
    let machine = Machine::new();
    let home = machine.home();
    for bare in [home.clone(), home.join("Desktop"), home.join("Downloads")] {
        assert_eq!(machine.place(&bare), OTHER, "{bare:?}");
    }
    for inside in ["Desktop/notes", "Downloads/a/b", "Documents", "Code"] {
        assert_eq!(machine.place(home.join(inside)), format!("home/{inside}"));
    }
    // Cloud drives are folders of projects too.
    assert_eq!(
        machine.place(home.join("OneDrive/work")),
        "home/OneDrive/work"
    );
}

#[test]
fn host_made_directories_are_other_only_in_their_own_format() {
    let machine = Machine::new();
    let home = machine.home();
    let uuid = "0a1b2c3d-1111-2222-3333-444455556666";
    let other = [
        "WorkBuddy/2026-11-12-10-11-12",
        "WorkBuddy/2026-11-12-10-11-12/deep/er",
        "WorkBuddy/automation-2026-11-12-10-11-12",
        "Support/Claude/scratch-workspaces/scratch-2026-11-12-0a1b2c",
        "Documents/Codex/2026-11-12/some-task",
        "Documents/Codex/2026-11-12/some-task/sub",
        &format!(".zcode/workspace/1731400000000-{uuid}"),
        ".zcode/workspace/default",
        "Documents/Trae/temp_20261112_01",
        "Documents/Trae-CN/temp_20261112_02",
        "Documents/deepseek-harness/default-workspace",
    ];
    for cwd in other {
        assert_eq!(machine.place(home.join(cwd)), OTHER, "{cwd}");
    }
    let projects = [
        "WorkBuddy/my-app",
        "WorkBuddy/2026-11-12",
        "WorkBuddy/2026-11-12-10-11-1x",
        "Support/Claude/scratch-workspaces/scratch-2026-11-12-0a1b2g",
        "Support/Claude/other/scratch-2026-11-12-0a1b2c",
        "Documents/Codex/2026-11-12",
        "Documents/Codex/notes/task",
        "Documents/Codex",
        &format!(".zcode/workspace/173140000000-{uuid}"),
        ".zcode/workspace/mine",
        "Documents/Trae/temp_2026112_01",
        "Documents/Trae/work",
        "Documents/deepseek-harness/other",
    ];
    for cwd in projects {
        assert_eq!(
            machine.place(home.join(cwd)),
            format!("home/{cwd}"),
            "{cwd}"
        );
    }
}

#[test]
fn a_repository_inside_a_host_directory_is_still_a_project() {
    let machine = Machine::new();
    machine.make("home/WorkBuddy/2026-11-12-10-11-12/.git");
    assert_eq!(
        machine.place(machine.home().join("WorkBuddy/2026-11-12-10-11-12/src")),
        "home/WorkBuddy/2026-11-12-10-11-12"
    );
}

#[cfg(unix)]
#[test]
fn symbolic_links_resolve_to_the_real_project() {
    let machine = Machine::new();
    machine.make("home/Code/app/.git");
    machine.make("home/Code/app/src");
    std::os::unix::fs::symlink(machine.home().join("Code/app"), machine.home().join("link"))
        .unwrap();
    assert_eq!(
        machine.place(machine.home().join("link/src")),
        "home/Code/app"
    );
}

#[test]
fn windows_spellings_name_one_place() {
    assert_eq!(plain_windows(r"\\?\c:\Users\a"), r"C:\Users\a");
    assert_eq!(plain_windows(r"\\?\UNC\srv\share\x"), r"\\srv\share\x");
    assert_eq!(plain_windows(r"D:\x"), r"D:\x");
    assert_eq!(plain_windows("/usr/x"), "/usr/x");
}

#[test]
fn fixed_aliases_apply_to_places_that_do_not_exist_yet() {
    let table = [("/tmp", "/private/tmp")];
    assert_eq!(
        align_with(PathBuf::from("/tmp/a/b"), &table),
        PathBuf::from("/private/tmp/a/b")
    );
    assert_eq!(
        align_with(PathBuf::from("/tmp2/a"), &table),
        PathBuf::from("/tmp2/a")
    );
}

#[test]
fn components_not_prefixes_decide_containment() {
    assert!(within(Path::new("/a/tmp/x"), Path::new("/a/tmp")));
    assert!(!within(Path::new("/a/tmp2/x"), Path::new("/a/tmp")));
    assert!(too_broad(Path::new("/"), None));
    assert!(too_broad(Path::new("/home/a"), Some(Path::new("/home/a"))));
    assert!(!too_broad(
        Path::new("/home/a/tmp"),
        Some(Path::new("/home/a"))
    ));
}

#[cfg(windows)]
#[test]
fn windows_paths_compare_without_case() {
    assert!(within(
        Path::new(r"C:\Users\A\App\src"),
        Path::new(r"c:\users\a\app")
    ));
    assert!(!within(
        Path::new(r"C:\Users\A\App2"),
        Path::new(r"c:\users\a\app")
    ));
    assert!(canonical(Path::new(r"\mnt\c\x")).is_none());
}

#[test]
fn a_removed_worktree_returns_to_the_repository_recorded_for_it() {
    let machine = Machine::new();
    machine.make("home/Code/app/.git/worktrees/wt");
    machine.write("home/Code/app/.git/worktrees/wt/commondir", "../..\n");
    machine.write("home/Code/wt/.git", "gitdir: ../app/.git/worktrees/wt\n");
    machine.make("home/Code/wt/src");
    let wt = canonical(&machine.base.join("home/Code/wt")).unwrap();
    let app = machine.base.join("home/Code/app");
    assert_eq!(machine.place(&wt), "home/Code/app");
    assert_eq!(
        machine.recorded().get(&key(&wt)),
        Some(&app.to_string_lossy().into_owned())
    );
    // Above the removed directory there is no `.git` any more; the record still names the
    // checkout, whose own directory may be gone as well.
    fs::remove_dir_all(&wt).unwrap();
    machine.restart();
    assert_eq!(machine.place(&wt), "home/Code/app");
    fs::remove_dir_all(&app).unwrap();
    assert_eq!(machine.place(&wt), "home/Code/app");
}

#[test]
fn records_of_directories_no_longer_listed_are_forgotten() {
    let machine = Machine::new();
    machine.make("home/Code/a/.git");
    machine.make("home/Code/b/.git");
    machine.make("home/Code/a/src");
    machine.make("home/Code/b/src");
    let a = canonical(&machine.base.join("home/Code/a/src")).unwrap();
    let b = canonical(&machine.base.join("home/Code/b/src")).unwrap();
    assert_eq!(machine.place(&a), "home/Code/a");
    assert_eq!(machine.place(&b), "home/Code/b");
    assert_eq!(machine.recorded().len(), 2);

    // An unchanged answer is not written again.
    fs::remove_file(&machine.records).unwrap();
    assert_eq!(machine.place(&a), "home/Code/a");
    assert!(!machine.records.exists());

    // Origins that could not be read forget nothing.
    machine.forget(&[]);
    assert!(machine.memory.borrow_mut().root(&b).is_some());

    // A directory no workspace starts in any more is dropped.
    machine.forget(std::slice::from_ref(&a));
    assert_eq!(
        machine.recorded(),
        BTreeMap::from([(
            key(&a),
            machine
                .base
                .join("home/Code/a")
                .to_string_lossy()
                .into_owned()
        )])
    );
    assert!(machine.memory.borrow_mut().root(&b).is_none());
}

#[test]
fn broad_candidates_are_not_scratch_places() {
    let root = tempfile::tempdir().unwrap();
    let home = canonical(root.path()).unwrap().join("home");
    fs::create_dir_all(&home).unwrap();
    let dirs = HostDirs {
        home: home.clone(),
        ..HostDirs::default()
    };
    // A temporary directory that is the home folder or a volume root would swallow everything.
    let env = Env::new(
        dirs,
        vec![home.clone(), PathBuf::from("/")],
        home.join(".agentnave"),
        Vec::new(),
    );
    assert!(!env.is_scratch(&home.join("Code/app")));
    assert!(env.is_scratch(&home.join(".agentnave/x")));
}

#[test]
fn a_repository_at_the_home_folder_does_not_swallow_what_is_below() {
    let machine = Machine::new();
    machine.make("home/.git");
    machine.make("home/Desktop/.git");
    machine.make("home/Code/plain");
    machine.make("home/Code/app/.git");
    assert_eq!(machine.place(machine.home()), OTHER);
    assert_eq!(machine.place(machine.home().join("Desktop")), OTHER);
    assert_eq!(
        machine.place(machine.home().join("Code/plain")),
        "home/Code/plain"
    );
    assert_eq!(
        machine.place(machine.home().join("Desktop/notes")),
        "home/Desktop/notes"
    );
    assert_eq!(
        machine.place(machine.home().join("Code/app/src")),
        "home/Code/app"
    );
}

#[test]
fn a_record_that_could_not_be_saved_is_written_on_the_next_try() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config");
    std::fs::write(&config, b"a file where the folder should be").unwrap();
    let mut memory = Memory::at(config.join(super::memory::FILE));
    let (cwd, root) = (dir.path().join("wt"), dir.path().join("repo"));
    memory.remember(&cwd, &root);
    std::fs::remove_file(&config).unwrap();
    memory.remember(&cwd, &root);
    assert!(config.join(super::memory::FILE).is_file());
}
