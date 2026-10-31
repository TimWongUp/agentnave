use super::*;
use std::io::{BufRead, BufReader, Write};
use std::os::windows::fs::OpenOptionsExt;
use std::process::{Child, Stdio};

/// The holder is a separate PowerShell process, as with scanners/readers on a real machine.
struct ExclusiveFile(Child);

impl ExclusiveFile {
    fn open(target: &Path) -> Self {
        let powershell = PathBuf::from(std::env::var_os("SystemRoot").unwrap())
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
        let mut child = Command::new(powershell)
            .args(["-NoProfile", "-Command", "$ErrorActionPreference='Stop'; $f=[IO.File]::Open($env:AGENTNAVE_LOCK_TARGET,'Open','Read','None'); [Console]::WriteLine('locked'); [Console]::ReadLine() > $null; $f.Dispose()"])
            .env("AGENTNAVE_LOCK_TARGET", target)
            .stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        assert_eq!(line.trim(), "locked");
        Self(child)
    }

    fn release(mut self) {
        writeln!(self.0.stdin.take().unwrap(), "release").unwrap();
        assert!(self.0.wait().unwrap().success());
    }
}

impl Drop for ExclusiveFile {
    fn drop(&mut self) {
        // Also release the exclusive handle after a failed assertion in the parent test.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn exclusive_windows_handle_keeps_manifest_valid() {
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("active.json");
    let old = json!({"version":"0.57.0", "bin":"old"});
    let new = json!({"version":"0.58.0", "bin":"new"});
    write_active_manifest(directory.path(), &old).unwrap();
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&target)
        .unwrap();
    assert!(write_active_manifest(directory.path(), &new).is_err());
    drop(lock);
    assert_eq!(
        serde_json::from_slice::<Value>(&std::fs::read(&target).unwrap()).unwrap(),
        old
    );
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    write_active_manifest(directory.path(), &new).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&std::fs::read(&target).unwrap()).unwrap(),
        new
    );
}

fn succeeds_after_release(base: &Path, value: &Value) {
    let holder = ExclusiveFile::open(&base.join("active.json"));
    // The first replace meets the exclusive handle; the 80ms retry follows its release.
    let release = thread::spawn(move || {
        thread::sleep(Duration::from_millis(40));
        holder.release();
    });
    let started = Instant::now();
    let result = write_active_manifest(base, value);
    release.join().unwrap();
    println!(
        "same write call after PowerShell release: {result:?}, elapsed={:?}",
        started.elapsed()
    );
    assert!(result.is_ok());
    assert!(started.elapsed() >= Duration::from_millis(80));
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(base.join("active.json")).unwrap()).unwrap(),
        *value
    );
    assert!(
        !base
            .join(format!("active.{}.tmp", std::process::id()))
            .exists()
    );
}

#[test]
fn powershell_release_allows_the_same_write_call_to_retry() {
    let directory = tempfile::tempdir().unwrap();
    write_active_manifest(directory.path(), &json!({"version":"old"})).unwrap();
    succeeds_after_release(directory.path(), &json!({"version":"new"}));
}

#[cfg(feature = "windows-live-tests")]
#[test]
fn windows_live_active_lock() {
    let base = dirs::data_dir()
        .unwrap()
        .join("io.github.timwongup.agentnave/service");
    let target = base.join("active.json");
    let original = fs::read(&target).unwrap();
    let value: Value = serde_json::from_slice(&original).unwrap();
    let holder = ExclusiveFile::open(&target);
    let result = write_active_manifest(&base, &value);
    holder.release();
    println!("exclusive PowerShell handle result: {result:?}");
    assert!(result.is_err());
    assert_eq!(fs::read(&target).unwrap(), original);
    assert!(
        !base
            .join(format!("active.{}.tmp", std::process::id()))
            .exists()
    );
    succeeds_after_release(&base, &value);
    println!("pointer preserved; staging removed; retry succeeded without a separate write call");
}
