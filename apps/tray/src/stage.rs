//! Read-only projections of Studio snapshots for the stage and manager chats.
mod group;

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::data::{Item, Response, Snapshot, valid_id};

mod approvals;
mod desk;
mod show;
mod summary;

use desk::Desk;
use show::identity_of;
use summary::line;

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or_default()
}

fn array<'a>(value: &'a Value, key: &str) -> impl DoubleEndedIterator<Item = &'a Value> {
    value[key].as_array().into_iter().flatten()
}

fn label(host: Option<&str>) -> &str {
    match host {
        Some("codex") => "Codex",
        Some("claude") => "Claude Code",
        Some("codebuddy") => "CodeBuddy",
        Some("workbuddy") => "WorkBuddy",
        Some("grok") => "Grok",
        Some("antigravity") => "Antigravity",
        Some("pi") => "Pi",
        Some("deepseek") => "DeepSeek Harness",
        Some("zcode") => "ZCode",
        Some("opencode") => "OpenCode",
        Some("gemini") => "Gemini CLI",
        Some("hermes") => "Hermes",
        _ => "未知 boss",
    }
}

fn color(group: &str) -> u64 {
    // FNV-1a, independent of process seeds and platform word size.
    group.bytes().fold(0xcbf29ce484222325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    }) % 8
}

/// Studio emits RFC3339 with microseconds. Compare instants, including offsets, not strings.
pub(crate) fn timestamp(time: &str) -> Option<i64> {
    if time.get(4..5) != Some("-")
        || time.get(7..8) != Some("-")
        || time.get(10..11) != Some("T")
        || time.get(13..14) != Some(":")
        || time.get(16..17) != Some(":")
    {
        return None;
    }
    let number = |start, end| time.get(start..end)?.parse::<i64>().ok();
    let (year, month, day) = (number(0, 4)?, number(5, 7)?, number(8, 10)?);
    let (hour, minute, second) = (number(11, 13)?, number(14, 16)?, number(17, 19)?);
    let leap = |y| y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let months = [
        31,
        if leap(year) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if !(1..=9999).contains(&year)
        || !(1..=12).contains(&month)
        || !(1..=months[(month - 1) as usize]).contains(&day)
        || !(0..=23).contains(&hour)
        || !(0..=59).contains(&minute)
        || !(0..=59).contains(&second)
    {
        return None;
    }
    let y = year - 1;
    let days = 365 * (year - 1970) + y / 4 - 1969 / 4 - y / 100 + 1969 / 100 + y / 400 - 1969 / 400
        + months[..(month - 1) as usize].iter().sum::<i64>()
        + day
        - 1;
    let suffix = time.get(19..)?;
    let zone = suffix.find(['Z', '+', '-'])?;
    let fraction = match &suffix[..zone] {
        "" => "",
        part => part
            .strip_prefix('.')
            .filter(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))?,
    };
    let fractional = format!("{fraction:0<6}").get(..6)?.parse::<i64>().ok()?;
    let offset = match &suffix[zone..] {
        "Z" => 0,
        zone => {
            if zone.len() != 6 || zone.get(3..4) != Some(":") {
                return None;
            }
            let hours = zone.get(1..3)?.parse::<i64>().ok()?;
            let minutes = zone.get(4..6)?.parse::<i64>().ok()?;
            if !(0..=23).contains(&hours) || !(0..=59).contains(&minutes) {
                return None;
            }
            (hours * 60 + minutes) * if zone.starts_with('-') { -1 } else { 1 }
        }
    };
    Some(((days * 24 + hour) * 3600 + minute * 60 + second - offset * 60) * 1_000_000 + fractional)
}

pub(crate) fn iso(time: i64) -> String {
    // Gregorian civil date from days since the Unix epoch.
    let seconds = time.div_euclid(1_000_000);
    let days = seconds.div_euclid(86400) + 719468;
    let era = days.div_euclid(146097);
    let doe = days - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    let daytime = seconds.rem_euclid(86400);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:06}Z",
        daytime / 3600,
        daytime / 60 % 60,
        daytime % 60,
        time.rem_euclid(1_000_000)
    )
}

fn event_time(time: &str, updated: &str) -> String {
    [time, updated]
        .into_iter()
        .find(|time| timestamp(time).is_some())
        .unwrap_or("1970-01-01T00:00:00Z")
        .to_owned()
}

fn urgency(status: &str) -> usize {
    match status {
        "running" => 3,
        "review" => 2,
        "failed" | "blocked" | "interrupted" => 1,
        _ => 0,
    }
}

fn conversation_status(snapshot: &Snapshot) -> &'static str {
    let view = &snapshot.view["director"];
    let statuses = [
        snapshot.item.status.as_str(),
        text(&view["round"], "status"),
    ]
    .into_iter()
    .chain(array(view, "tasks").map(|task| text(task, "status")))
    .chain(array(&view["round"], "seats").map(|seat| {
        if seat["review"] == "pending" {
            "review"
        } else {
            text(seat, "status")
        }
    }));
    ["idle", "failed", "review", "running"][statuses.map(urgency).max().unwrap_or(0)]
}

fn active(status: &str) -> bool {
    matches!(status, "running" | "review")
}

fn member_status(status: &str) -> &str {
    match status {
        "running" | "review" | "succeeded" | "failed" | "blocked" | "cancelled" | "interrupted" => {
            status
        }
        _ => "pending",
    }
}

fn failure(status: &str) -> &str {
    match status {
        "failed" => "失败",
        "blocked" => "受阻",
        "cancelled" => "已取消",
        "interrupted" => "已中断",
        _ => "等待中",
    }
}

fn opt_text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

/// The caller's concrete responsibility, independent of its configured seat.
fn desk_role(identity: &Value) -> Option<String> {
    opt_text(identity, "role")
}

/// A desk is one native session: a private thread, or one seat of a saved show.
fn task_desk(conversation: &str, task: &Value, task_id: &str) -> String {
    match opt_text(task, "thread") {
        Some(thread) => format!("thread:{thread}"),
        // No native session yet (a new thread still running, or one that never started).
        None => format!("task:{conversation}:{task_id}"),
    }
}

fn seat_desk(conversation: &str, seat: &str) -> String {
    format!("seat:{conversation}:{seat}")
}

/// The core `Host.provider` mapping: which CLI a host is excluded from in private tasks.
fn host_provider(host: Option<&str>) -> Option<&str> {
    match host {
        Some("workbuddy") => Some("codebuddy"),
        Some("opencode") | Some("hermes") | None => None,
        Some(name) => Some(name),
    }
}

/// The desk of a seat identity, which carries its seat id under `seat`.
fn desk_of(conversation: &str, identity: &Value) -> Option<String> {
    (!text(identity, "provider").is_empty())
        .then(|| seat_desk(conversation, text(identity, "seat")))
}

fn desk_thread(key: &str) -> Value {
    json!({"kind": "desk", "key": key})
}

fn room_thread(id: &str) -> Value {
    json!({"kind": "room", "id": id})
}

/// Fill a projected message's office fields; every message carries all of them.
fn annotate(
    message: &mut Value,
    visibility: &str,
    thread: Value,
    desk: Option<&str>,
    message_id: Option<&str>,
    forward: Option<Value>,
) {
    message["visibility"] = json!(visibility);
    message["thread"] = thread;
    message["desk"] = desk.map_or(Value::Null, |desk| json!(desk));
    message["message_id"] = message_id.map_or(Value::Null, |id| json!(id));
    message["forward"] = forward.unwrap_or(Value::Null);
}

fn speaker(role: &str, provider: &str, model: &str, name: &str) -> Value {
    json!({"role": role, "provider": provider, "model": model, "label": name})
}

fn model(identity: &Value) -> &str {
    let reported = text(identity, "reported_model");
    if !reported.is_empty() {
        return reported;
    }
    match text(identity, "model") {
        "原生设置" | "继承原生设置" => "",
        value => value,
    }
}

fn member_speaker(identity: &Value) -> Value {
    let provider = text(identity, "provider");
    let mut who = speaker("member", provider, model(identity), label(Some(provider)));
    who["effort"] = identity["effort"].clone();
    who["identity"] = identity["identity"].clone();
    who["handle"] = identity["identity"]["handle"].clone();
    who
}

/// The boss who said this, kept through a handover; records older than `manager_host` fall back
/// to the host that opened the conversation. Only a boss that took over is named by its host.
fn manager_speaker(identity: &Value, opened_by: Option<&str>, name: &str) -> Value {
    let provider = opt_text(identity, "manager_host");
    let name = provider
        .as_deref()
        .filter(|host| Some(*host) != opened_by)
        .map_or(name, |host| label(Some(host)));
    let provider = provider.as_deref().or(opened_by).unwrap_or("");
    let reported = text(identity, "manager_reported_model");
    let mut who = speaker(
        "manager",
        provider,
        if reported.is_empty() {
            text(identity, "manager_model")
        } else {
            reported
        },
        name,
    );
    who["model_source"] = json!(if reported.is_empty() {
        "declared"
    } else {
        "native"
    });
    who["handle"] = json!("@boss");
    who
}

fn recipient(desk: &str, identity: &Value) -> Value {
    let provider = text(identity, "provider");
    let model = text(identity, "model");
    json!({"key": desk, "provider": provider, "model": model,
        "label": label(Some(provider)), "seat": text(identity, "seat"),
        "effort": identity["effort"], "identity": identity["identity"],
        "handle": identity["identity"]["handle"]})
}

/// A `to` chip: whom a message addresses, as the service named them for that message.
fn addressee(recipient: &Value) -> Value {
    json!({"key": recipient["key"], "handle": recipient["handle"],
        "effort": recipient["effort"], "identity": recipient["identity"]})
}

/// The desk a committed entry belongs to: the speaking seat, or a single task recipient.
fn entry_desk(conversation: &str, entry: &Value, seats: &BTreeMap<&str, &Value>) -> Option<String> {
    let seat_desk = |seat_id: &str| {
        seats
            .get(seat_id)
            .filter(|seat| !text(seat, "provider").is_empty())
            .map(|_| seat_desk(conversation, seat_id))
    };
    match text(entry, "kind") {
        "task" => {
            let recipients = entry["recipients"].as_array()?;
            if recipients.len() != 1 {
                return None;
            }
            seat_desk(recipients[0]["seat"].as_str()?)
        }
        "host" | "forward" => None,
        // Answers, absences and unknown kinds still name their speaker's desk.
        _ => seat_desk(text(entry, "speaker")),
    }
}

fn route_to(message: &mut Value, mut recipients: Vec<Value>) {
    let conversation = text(message, "conversation").to_owned();
    for target in &mut recipients {
        if !text(target, "seat").is_empty() {
            target["focus"] = json!(format!("seat:{conversation}:{}", text(target, "seat")));
        }
    }
    message["to"] = json!(recipients.iter().map(addressee).collect::<Vec<_>>());
    message["recipients"] = json!(recipients);
}

struct Member {
    value: Value,
    at: i64,
}

/// A workspace's handover (ADR-0033): waiting for another boss, or led by a boss other than
/// the one that created it (`host`, when known).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Handover {
    pub pending: bool,
    pub changed: bool,
    pub host: Option<String>,
    pub note: Option<String>,
    pub released: Option<String>,
    pub previous_host: Option<String>,
    /// Name the boss gave when handing over, for a workspace with no conversation to name it.
    pub title: Option<String>,
    pub cwd: Option<String>,
}

struct Group {
    id: String,
    host: Option<String>,
    handover: Handover,
    /// The boss chats in Claude Desktop's Code tab: shown as its own app, still the `claude` host.
    app: Option<&'static str>,
    updated: String,
    at: i64,
    title: String,
    /// The earliest conversation's creation time, title and id; its title names the workspace,
    /// so renaming the workspace renames that conversation.
    first_title: Option<(i64, String, String)>,
    /// The earliest conversation of all, renamed when no title names the workspace yet.
    first_id: Option<(i64, String)>,
    project_name: Option<String>,
    live: bool,
    status: &'static str,
    latest_end: Option<(i64, String)>,
    color: u64,
    conversations: Vec<Value>,
    members: BTreeMap<String, Member>,
    desks: BTreeMap<String, Desk>,
    rooms: Vec<(i64, String, Value)>,
    messages: Vec<Value>,
    typing: BTreeMap<String, (i64, Value)>,
    sources: Vec<usize>,
    include_messages: bool,
}

/// The mark and name of a Claude or Codex boss, by the app its session records name (see
/// `data::boss_app`); neither name is claimed for a boss whose app is unknown.
fn boss_name(
    host: Option<&str>,
    app: Option<&str>,
) -> (Option<&'static str>, Option<&'static str>) {
    use crate::data::boss_app::{CLAUDE_CLI, CLAUDE_DESKTOP, CODEX_CLI, CODEX_DESKTOP};
    match (host, app) {
        (Some("claude"), Some(CLAUDE_DESKTOP)) => (Some(CLAUDE_DESKTOP), Some("Claude 桌面")),
        (Some("claude"), Some(CLAUDE_CLI)) => (None, Some("Claude Code")),
        (Some("claude"), _) => (None, Some("Claude")),
        (Some("codex"), Some(CODEX_DESKTOP)) => (Some(CODEX_DESKTOP), Some("Codex 桌面")),
        (Some("codex"), Some(CODEX_CLI)) => (None, Some("Codex CLI")),
        _ => (None, None),
    }
}

/// Conversations with the same boss chat share a workspace.
pub(crate) fn chat_group(item: &Item) -> Option<String> {
    let id = item.manager_session.as_ref()?;
    valid_id(id).then(|| id.clone())
}

/// One snapshot's identity for reuse: its view's address, and whether and when it is live.
/// Unchanged saved views keep their parsed Arc, so the address changes with the content.
pub(crate) type SourceKey = (usize, bool, Option<String>);

pub(crate) fn source_key(snapshot: &Snapshot) -> SourceKey {
    (
        Arc::as_ptr(&snapshot.view) as usize,
        snapshot.live,
        snapshot.live_updated.clone(),
    )
}

/// The chat list's previews by group, with the views each was computed from (held so their
/// addresses stay unique); only groups whose snapshots changed are projected again.
type Preview = ((Vec<SourceKey>, Handover), Vec<Arc<Value>>, String);
static PREVIEWS: LazyLock<Mutex<HashMap<String, Preview>>> = LazyLock::new(Mutex::default);

/// At most this many undrawn desks are listed; the count still covers all of them.
const OPTIMIZED_LISTED: usize = 200;

/// Desks the returned seats and messages name, as speaker, thread or recipient.
fn referenced_desks(rooms: &[Value], messages: &[Value]) -> BTreeSet<String> {
    let seats = rooms.iter().flat_map(|room| array(room, "seats"));
    let named = messages.iter().flat_map(|message| {
        let thread = (message["thread"]["kind"] == "desk").then(|| &message["thread"]["key"]);
        [&message["desk"]]
            .into_iter()
            .chain(thread)
            .chain(array(message, "to").map(|to| &to["key"]))
            .chain(array(message, "recipients").map(|to| &to["key"]))
    });
    seats
        .map(|seat| &seat["desk"])
        .chain(named)
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

/// `limit` is the App's running limit (`settings.json`), which also caps the desks drawn.
pub(crate) fn respond(
    path: &str,
    mut snapshots: Vec<Snapshot>,
    data_exists: bool,
    limit: usize,
    handovers: &HashMap<String, Handover>,
    apps: &HashMap<String, &'static str>,
) -> Response {
    let requested = path.strip_prefix("/app/chats/");
    let complete = requested.is_some_and(|path| path.ends_with("/history"));
    let requested = requested.map(|path| path.strip_suffix("/history").unwrap_or(path));
    let (requested, focus) = match requested {
        Some(path) => match path.split_once("/focus/") {
            Some((group, conversation)) => (Some(group), Some(conversation)),
            None => (Some(path), None),
        },
        None => (None, None),
    };
    if requested.is_some_and(|id| !valid_id(id)) || focus.is_some_and(|id| !valid_id(id)) {
        return Response::error(404, "not found");
    }
    let (mut groups, stage_ids) = projected_groups(&mut snapshots, handovers, apps);
    if let Some(id) = requested {
        let Some(group) = groups.iter_mut().find(|g| g.id == id) else {
            return Response::error(404, "not found");
        };
        if focus.is_some_and(|id| !group.conversations.iter().any(|c| c["id"] == id)) {
            return Response::error(404, "not found");
        }
        group.project(&snapshots, true);
        let host = group.host.clone();
        let mut value = group.header();
        value.as_object_mut().unwrap().remove("updated");
        value["conversations"] = json!(group.conversations);
        value["members"] = json!(group.members());
        let rooms = group.rooms();
        let messages = group.timeline(focus, complete);
        let referenced = referenced_desks(&rooms, &messages);
        let (desks, optimized) = group.desks(host.as_deref(), limit, OPTIMIZED_LISTED, &referenced);
        value["desks"] = json!(desks);
        value["optimized"] = optimized;
        value["rooms"] = json!(rooms);
        value["messages"] = json!(messages);
        value["boss_messages"] = json!(group.boss_messages());
        value["omitted"] = json!(group.omitted(focus, complete));
        value["threads"] = json!(group.threads());
        return Response::json(&value);
    }
    if path == "/app/stage" {
        panel_response(groups, &snapshots, &stage_ids, data_exists, limit)
    } else {
        list_response(groups, &snapshots)
    }
}

/// Whether a conversation title was written by the boss, not the `<provider> 任务` default.
fn meaningful_title(title: &str) -> bool {
    let title = title.trim();
    !title.is_empty()
        && !matches!(
            title.strip_suffix(" 任务"),
            Some("antigravity" | "claude" | "codebuddy" | "codex" | "deepseek" | "grok" | "pi")
        )
}

/// The project a working directory belongs to: its folder name, or the repository's name for an
/// agent worktree such as `<repo>/.claude/worktrees/<name>`.
fn project_name(cwd: &str) -> Option<String> {
    let parts: Vec<&str> = cwd.split(['/', '\\']).filter(|p| !p.is_empty()).collect();
    match parts.as_slice() {
        [.., repo, dot, "worktrees", _] if dot.starts_with('.') => Some((*repo).to_owned()),
        [.., last] => Some((*last).to_owned()),
        [] => None,
    }
}

#[cfg(test)]
mod tests;

fn projected_groups(
    snapshots: &mut [Snapshot],
    handovers: &HashMap<String, Handover>,
    apps: &HashMap<String, &'static str>,
) -> (Vec<Group>, Vec<String>) {
    snapshots.sort_by_key(|s| {
        (
            timestamp(&s.item.updated).unwrap_or_default(),
            s.item.id.clone(),
        )
    });
    let mut groups = BTreeMap::new();
    for (index, snapshot) in snapshots.iter().enumerate() {
        let Some(id) = chat_group(&snapshot.item) else {
            continue;
        };
        groups
            .entry(id.clone())
            .or_insert_with(|| Group::new(id))
            .add(snapshot, index);
    }
    for (id, handover) in handovers {
        if handover.title.is_some() && valid_id(id) && !groups.contains_key(id) {
            groups.insert(id.clone(), Group::only_boss(id.clone(), handover));
        }
    }
    let mut groups: Vec<_> = groups.into_values().collect();
    for group in &mut groups {
        group.project(snapshots, false);
        if let Some(handover) = handovers.get(&group.id) {
            group.handover = handover.clone();
            if group.first_title.is_none()
                && let Some(title) = &handover.title
            {
                group.title.clone_from(title);
            }
        }
        group.app = apps.get(&group.id).copied();
    }
    // Work going on first, then the most recent; never empty while history exists. A failure is
    // no reason to come first: the boss handles it, the user has nothing to do.
    let tier = |status: &str| match status {
        "running" | "review" => 0,
        _ => 1,
    };
    groups.sort_by(|a, b| {
        (tier(a.status), Reverse(a.at), &a.id).cmp(&(tier(b.status), Reverse(b.at), &b.id))
    });
    let stage_ids: Vec<_> = groups.iter().take(5).map(|g| g.id.clone()).collect();
    let mut ids = stage_ids.clone();
    ids.sort();
    let mut used = [false; 8];
    for id in ids {
        let group = groups.iter_mut().find(|g| g.id == id).unwrap();
        let mut assigned = color(&id) as usize;
        while used[assigned] {
            assigned = (assigned + 1) % 8;
        }
        used[assigned] = true;
        group.color = assigned as u64;
    }
    (groups, stage_ids)
}

fn panel_response(
    groups: Vec<Group>,
    snapshots: &[Snapshot],
    stage_ids: &[String],
    data_exists: bool,
    limit: usize,
) -> Response {
    let managers: Vec<_> = groups
        .into_iter()
        .filter(|g| stage_ids.contains(&g.id))
        .map(|mut g| {
            g.project(snapshots, true);
            let mut value = g.header();
            value["threads"] = json!(g.threads());
            value["omitted"] = json!(g.omitted(None, false));
            value["members"] = json!(g.members());
            let rooms = g.rooms();
            // The panel only shows how many; the office lists them. Seats still need theirs.
            let referenced = referenced_desks(&rooms, &[]);
            let (desks, optimized) = g.desks(g.host.as_deref(), limit, 0, &referenced);
            value["desks"] = json!(desks);
            value["optimized"] = optimized;
            value["rooms"] = json!(rooms);
            value
        })
        .collect();
    let counts = snapshots
        .iter()
        .filter(|s| chat_group(&s.item).is_some_and(|id| stage_ids.contains(&id)))
        .map(crate::data::call_counts)
        .fold((0, 0), |(running, review), (r, v)| {
            (running + r, review + v)
        });
    let (running, review) = counts;
    let empty = if !managers.is_empty() {
        None
    } else if !data_exists {
        Some("no_data")
    } else {
        Some("quiet")
    };
    Response::json(
        &json!({"version": 1, "counts": {"running": running, "review": review}, "managers": managers, "empty": empty}),
    )
}

fn list_response(groups: Vec<Group>, snapshots: &[Snapshot]) -> Response {
    let mut previews = PREVIEWS.lock().unwrap_or_else(|p| p.into_inner());
    let mut kept = HashMap::new();
    let groups: Vec<_> = groups
        .into_iter()
        .take(200)
        .map(|mut g| {
            let key = (
                g.sources
                    .iter()
                    .map(|&i| source_key(&snapshots[i]))
                    .collect(),
                g.handover.clone(),
            );
            let preview = match previews.remove(&g.id) {
                Some(preview) if preview.0 == key => preview,
                _ => {
                    g.project(snapshots, true);
                    let text = g
                        .timeline(None, false)
                        .last()
                        .map(|m| line("succeeded", text(m, "text"), ""))
                        .unwrap_or_default();
                    let views = g
                        .sources
                        .iter()
                        .map(|&i| snapshots[i].view.clone())
                        .collect();
                    (key, views, text)
                }
            };
            let mut value = g.header();
            value["title"] = json!(g.title);
            value["conversations"] = json!(g.conversations.len());
            value["desks_count"] = json!(g.desks.len());
            value["rooms_count"] = json!(g.rooms.len());
            value["preview"] = json!(preview.2);
            kept.insert(g.id.clone(), preview);
            value
        })
        .collect();
    *previews = kept;
    Response::json(&json!({"groups": groups}))
}
