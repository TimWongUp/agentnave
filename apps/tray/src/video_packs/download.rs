#[cfg(test)]
mod tests;

use super::*;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::{
    Arc, Mutex, MutexGuard, OnceLock,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

type Key = (PathBuf, String);
struct Job {
    cancel: Arc<AtomicBool>,
    state: Value,
}
static JOBS: OnceLock<Mutex<HashMap<Key, Job>>> = OnceLock::new();

fn jobs() -> MutexGuard<'static, HashMap<Key, Job>> {
    JOBS.get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}
fn key(shared: &Path, tier: &str) -> Key {
    (write_root(shared), tier.into())
}

pub(super) fn status(shared: &Path, tier: &str) -> Value {
    jobs()
        .get(&key(shared, tier))
        .map_or(Value::Null, |job| job.state.clone())
}

pub(super) fn check_cancel(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Acquire) {
        Err("下载已取消".into())
    } else {
        Ok(())
    }
}

fn update(key: &Key, cancel: &Arc<AtomicBool>, apply: impl FnOnce(&mut Value)) {
    let mut jobs = jobs();
    if let Some(job) = jobs.get_mut(key)
        && Arc::ptr_eq(&job.cancel, cancel)
    {
        apply(&mut job.state);
    }
}

fn cancel_locked(jobs: &mut HashMap<Key, Job>, key: &Key) {
    if let Some(job) = jobs.remove(key) {
        job.cancel.store(true, Ordering::Release);
    }
}

pub(super) fn cancel(shared: &Path, tier: &str) {
    cancel_locked(&mut jobs(), &key(shared, tier));
}

pub(super) fn delete(shared: &Path, tier: &str) -> Result<(), String> {
    let key = key(shared, tier);
    let mut guard = jobs();
    cancel_locked(&mut guard, &key);
    let base = key.0.join("video-packs");
    let _ = fs::remove_file(part_path(&base, tier));
    let target = base.join(tier);
    if target.exists() {
        let old = tempfile::tempdir_in(base).map_err(|e| e.to_string())?;
        fs::rename(target, old.path().join("pack")).map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub(super) fn import_dir(shared: &Path, tier: &str, chosen: &Path) -> Result<(), String> {
    let base = write_root(shared).join("video-packs");
    let staged = install::stage(&base, tier, chosen)?;
    let mut guard = jobs();
    cancel_locked(&mut guard, &key(shared, tier));
    let _ = fs::remove_file(part_path(&base, tier));
    install::swap(&base, tier, staged)
}

fn part_path(base: &Path, tier: &str) -> PathBuf {
    base.join(format!(".{tier}.tar.gz.part"))
}

pub(super) fn start(shared: &Path, tier: &str) -> Result<(), String> {
    let key = key(shared, tier);
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut guard = jobs();
        if guard
            .get(&key)
            .is_some_and(|job| job.state["state"] != "failed")
        {
            return Ok(());
        }
        guard.insert(
            key.clone(),
            Job {
                cancel: cancel.clone(),
                state: json!({
                    "state": "downloading", "downloaded": 0, "total": null, "error": null
                }),
            },
        );
    }
    let worker_key = key.clone();
    let worker_cancel = cancel.clone();
    if let Err(error) = std::thread::Builder::new()
        .name(format!("video-pack-{tier}"))
        .spawn(move || {
            let result = run(&worker_key, &worker_cancel);
            let mut guard = jobs();
            if guard
                .get(&worker_key)
                .is_some_and(|job| Arc::ptr_eq(&job.cancel, &worker_cancel))
            {
                match result {
                    Ok(()) => {
                        guard.remove(&worker_key);
                    }
                    Err(error) => {
                        let job = guard.get_mut(&worker_key).unwrap();
                        job.state["state"] = json!("failed");
                        job.state["error"] = json!(error);
                    }
                }
            }
        })
    {
        update(&key, &cancel, |state| {
            state["state"] = json!("failed");
            state["error"] = json!(error.to_string());
        });
        return Err(error.to_string());
    }
    Ok(())
}

fn archive_name(tier: &str) -> String {
    if CLIP_EXT == "mov" {
        format!("agentnave-video-{tier}.tar.gz")
    } else {
        format!("agentnave-video-webm-{tier}.tar.gz")
    }
}

fn urls(tier: &str) -> Vec<String> {
    if let Ok(url) = std::env::var(format!("AGENTNAVE_VIDEO_PACK_URL_{tier}"))
        && !url.trim().is_empty()
    {
        return crate::proxy::expand_urls(&[url]);
    }
    // The packs live on one fixed release so app releases do not have to carry them again.
    let name = archive_name(tier);
    let url = format!("https://github.com/TimWongUp/agentnave/releases/download/v1.4.0/{name}");
    crate::proxy::expand_urls(&[url])
}

fn open_response(
    client: &reqwest::blocking::Client,
    url: &str,
    output: &mut fs::File,
    cancel: &AtomicBool,
) -> Result<(reqwest::blocking::Response, u64), String> {
    use std::io::{Seek, SeekFrom};
    check_cancel(cancel)?;
    let mut have = output.metadata().map_or(0, |m| m.len());
    let limit = 221 * install::CLIP_LIMIT;
    if have > limit {
        output.set_len(0).map_err(|e| e.to_string())?;
        have = 0;
    }
    let send = |offset: u64| {
        let mut req = client.get(url);
        if offset > 0 {
            req = req.header(reqwest::header::RANGE, format!("bytes={offset}-"));
        }
        req.send().map_err(|e| e.to_string())
    };
    let mut response = send(have)?;
    check_cancel(cancel)?;
    if have > 0 && response.status() == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
        output.set_len(0).map_err(|e| e.to_string())?;
        have = 0;
        response = send(0)?;
        check_cancel(cancel)?;
    }
    let response = response.error_for_status().map_err(|e| e.to_string())?;
    if have > 0 && response.status() == reqwest::StatusCode::PARTIAL_CONTENT {
        output.seek(SeekFrom::End(0)).map_err(|e| e.to_string())?;
        Ok((response, have))
    } else {
        output.set_len(0).map_err(|e| e.to_string())?;
        output.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
        Ok((response, 0))
    }
}

fn transfer(
    client: &reqwest::blocking::Client,
    url: &str,
    output: &mut fs::File,
    key: &Key,
    cancel: &Arc<AtomicBool>,
) -> Result<(), String> {
    let (mut response, mut downloaded) = open_response(client, url, output, cancel)?;
    let total = response.content_length().map(|len| downloaded + len);
    let limit = 221 * install::CLIP_LIMIT;
    if total.is_some_and(|total| total > limit) {
        return Err("视频包下载超过大小限制".into());
    }
    update(key, cancel, |state| {
        state["downloaded"] = json!(downloaded);
        state["total"] = json!(total);
    });
    let mut buffer = [0; 65536];
    loop {
        check_cancel(cancel)?;
        let n = response.read(&mut buffer).map_err(|e| e.to_string())?;
        check_cancel(cancel)?;
        if n == 0 {
            break;
        }
        downloaded += n as u64;
        if downloaded > limit {
            return Err("视频包下载超过大小限制".into());
        }
        output.write_all(&buffer[..n]).map_err(|e| e.to_string())?;
        update(key, cancel, |state| state["downloaded"] = json!(downloaded));
    }
    output.flush().map_err(|e| e.to_string())
}

fn build_client() -> Result<reqwest::blocking::Client, String> {
    let mut builder = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30 * 60));
    if let Some(proxy_url) = crate::proxy::resolve(None)
        && let Ok(proxy) = reqwest::Proxy::all(proxy_url.as_str())
    {
        builder = builder.proxy(proxy);
    }
    builder.build().map_err(|e| e.to_string())
}

fn run(key: &Key, cancel: &Arc<AtomicBool>) -> Result<(), String> {
    let base = key.0.join("video-packs");
    fs::create_dir_all(&base).map_err(|e| e.to_string())?;
    let part = part_path(&base, &key.1);
    let mut archive = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&part)
        .map_err(|e| e.to_string())?;
    let client = build_client()?;
    let mut last = Err("无法下载视频包".into());
    for url in urls(&key.1) {
        check_cancel(cancel)?;
        last = transfer(&client, &url, &mut archive, key, cancel);
        if last.is_ok() {
            break;
        }
    }
    last?;
    drop(archive);
    check_cancel(cancel)?;
    update(key, cancel, |state| state["state"] = json!("extracting"));
    let extracted = tempfile::tempdir_in(&base).map_err(|e| e.to_string())?;
    let staged = match install::extract(&part, extracted.path(), cancel)
        .and_then(|()| install::stage(&base, &key.1, extracted.path()))
    {
        Ok(staged) => staged,
        Err(error) => {
            if !cancel.load(Ordering::Acquire) {
                let _ = fs::remove_file(&part);
            }
            return Err(error);
        }
    };
    // Cancellation, deletion, import and the final swap share this lock: an obsolete worker
    // can never reinstall a pack after a successful cancel/delete/import response.
    let guard = jobs();
    check_cancel(cancel)?;
    if !guard
        .get(key)
        .is_some_and(|job| Arc::ptr_eq(&job.cancel, cancel))
    {
        return Err("下载已取消".into());
    }
    let result = install::swap(&base, &key.1, staged);
    if result.is_ok() {
        let _ = fs::remove_file(&part);
    }
    result
}
