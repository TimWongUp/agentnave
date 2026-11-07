use super::*;
use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::Instant;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum Channel {
    Npm(String),
    Brew {
        program: PathBuf,
        name: String,
        cask: bool,
    },
    Grok(String),
    Trae(String),
}

type Entries = HashMap<Channel, (Instant, Option<String>)>;
static CACHE: LazyLock<Mutex<Entries>> = LazyLock::new(|| Mutex::new(HashMap::new()));

pub(super) fn cached(channel: &Channel) -> Option<String> {
    cached_with(&CACHE, channel, Instant::now(), query)
}

pub(super) fn cached_with(
    cache: &Mutex<Entries>,
    channel: &Channel,
    now: Instant,
    query: impl FnOnce(&Channel) -> Option<String>,
) -> Option<String> {
    {
        let cache = cache.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((at, value)) = cache.get(channel) {
            let ttl = if value.is_some() {
                6 * 60 * 60
            } else {
                15 * 60
            };
            if now.saturating_duration_since(*at) < Duration::from_secs(ttl) {
                return value.clone();
            }
        }
    }
    let value = query(channel).filter(|v| version(v).is_some());
    cache
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(channel.clone(), (now, value.clone()));
    value
}

fn http(url: &str) -> Option<String> {
    // Match the tray's reqwest client (voice); updater also uses ring explicitly.
    let _ = rustls::crypto::ring::default_provider().install_default();
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(8))
        .build()
        .ok()?
        .get(url)
        .send()
        .ok()?
        .error_for_status()
        .ok()?
        .text()
        .ok()
}

fn query(channel: &Channel) -> Option<String> {
    match channel {
        Channel::Npm(package) => parse_npm(&http(&format!(
            "https://registry.npmjs.org/{package}/latest"
        ))?),
        Channel::Grok(channel) => grok_with(channel, http),
        Channel::Trae(track) => parse_trae(
            &http(&format!(
                "https://lf-cdn.trae.com.cn/obj/trae-com-cn/trae-cli/v2/public/{track}/latest.json"
            ))?,
            track,
        ),
        Channel::Brew {
            program,
            name,
            cask,
        } => {
            let mut args = vec!["info", "--json=v2"];
            if *cask {
                args.push("--cask");
            }
            args.push(name);
            let mut command = super::super::probe::command(program, &args);
            command
                .stdin(Stdio::null())
                .env("NO_COLOR", "1")
                .env("HOMEBREW_NO_AUTO_UPDATE", "1");
            let output = crate::process::output(command, Duration::from_secs(15)).ok()?;
            if !output.status.success() {
                return None;
            }
            parse_brew(&String::from_utf8_lossy(&output.stdout), *cask)
        }
    }
}

pub(super) fn grok_with(
    channel: &str,
    mut get: impl FnMut(&str) -> Option<String>,
) -> Option<String> {
    [
        "https://x.ai/cli",
        "https://storage.googleapis.com/grok-build-public-artifacts/cli",
    ]
    .iter()
    .find_map(|base| {
        let text = get(&format!("{base}/{channel}"))?;
        let v = text.trim();
        version(v).map(|_| v.to_owned())
    })
}

pub(super) fn parse_trae(text: &str, track: &str) -> Option<String> {
    let value: Value = serde_json::from_str(text).ok()?;
    if value["channel"] != "public"
        || value["distribution"] != "public"
        || value["release_channel"] != track
    {
        return None;
    }
    let version = value["version"].as_str()?;
    super::super::version_of(version)
}

pub(super) fn parse_npm(text: &str) -> Option<String> {
    let value: Value = serde_json::from_str(text).ok()?;
    let v = value["version"].as_str()?.trim();
    version(v).map(|_| v.to_owned())
}

pub(super) fn parse_brew(text: &str, cask: bool) -> Option<String> {
    let value: Value = serde_json::from_str(text).ok()?;
    let v = if cask {
        value["casks"][0]["version"].as_str()?.split(',').next()?
    } else {
        value["formulae"][0]["versions"]["stable"].as_str()?
    };
    version(v.trim()).map(|_| v.trim().to_owned())
}

fn version(raw: &str) -> Option<(Vec<u64>, Option<&str>)> {
    let raw = raw
        .trim()
        .strip_prefix('v')
        .unwrap_or(raw.trim())
        .split('+')
        .next()?;
    let (digits, pre) = raw
        .split_once('-')
        .map_or((raw, None), |(a, b)| (a, Some(b)));
    let digits = digits
        .split('.')
        .map(|p| {
            (!p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
                .then(|| p.parse::<u64>().ok())
                .flatten()
        })
        .collect::<Option<Vec<_>>>()?;
    if digits.len() < 2
        || pre.is_some_and(|p| {
            p.is_empty()
                || !p
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
        })
    {
        return None;
    }
    Some((digits, pre))
}

/// Numeric release segments, then SemVer prerelease ordering; missing trailing zeros are equal.
pub(super) fn newer(latest: &str, current: &str) -> bool {
    use std::cmp::Ordering;
    let (Some((a, ap)), Some((b, bp))) = (version(latest), version(current)) else {
        return false;
    };
    for i in 0..a.len().max(b.len()) {
        match a.get(i).unwrap_or(&0).cmp(b.get(i).unwrap_or(&0)) {
            Ordering::Equal => {}
            order => return order.is_gt(),
        }
    }
    match (ap, bp) {
        (None, Some(_)) => true,
        (Some(_), None) | (None, None) => false,
        (Some(a), Some(b)) => {
            let a: Vec<_> = a.split('.').collect();
            let b: Vec<_> = b.split('.').collect();
            for (a, b) in a.iter().zip(&b) {
                let order = match (a.parse::<u64>(), b.parse::<u64>()) {
                    (Ok(a), Ok(b)) => a.cmp(&b),
                    (Ok(_), Err(_)) => Ordering::Less,
                    (Err(_), Ok(_)) => Ordering::Greater,
                    _ => a.cmp(b),
                };
                if order != Ordering::Equal {
                    return order.is_gt();
                }
            }
            a.len() > b.len()
        }
    }
}
