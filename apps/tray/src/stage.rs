//! Read-only projections of Studio snapshots for the stage and manager chats.
mod group;
mod origin;

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::data::snapshot::{Item, Snapshot};
use crate::data_files::valid_id;
use crate::isolated::{self, Dir};
use crate::projects::Project;
use crate::response::Response;
use crate::time_format::{iso, timestamp};

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
        Some("devin") => "Devin",
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

fn cli_label<'a>(identity: &'a Value, provider: &'a str) -> &'a str {
    let named = text(&identity["identity"], "cli");
    if named.is_empty() {
        label(Some(provider))
    } else {
        named
    }
}

fn member_speaker(identity: &Value) -> Value {
    let provider = text(identity, "provider");
    let mut who = speaker(
        "member",
        provider,
        model(identity),
        cli_label(identity, provider),
    );
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
        "label": cli_label(identity, provider), "seat": text(identity, "seat"),
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
    /// The earliest dispatch's directory; its folder names the workspace when nothing else does.
    origin: Option<origin::Origin>,
    project_name: Option<String>,
    /// The project of that directory, once known.
    project: Option<Project>,
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
    /// The isolated directories of this workspace.
    isolated: Vec<Value>,
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
/// How many desks besides the running ones the panel receives: its cards show four, the idle
/// rows pick up to eight faces from them.
const PANEL_IDLE_DESKS: usize = 8;

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

/// If `path` asks for a single valid workspace chat route, return that workspace's group ID.
pub(crate) fn requested_group(path: &str) -> Option<&str> {
    let rest = path.strip_prefix("/app/chats/")?;
    let rest = rest.strip_suffix("/history").unwrap_or(rest);
    let (group, focus) = match rest.split_once("/focus/") {
        Some((group, focus)) => (group, Some(focus)),
        None => (rest, None),
    };
    (valid_id(group) && focus.is_none_or(valid_id)).then_some(group)
}

/// Top five workspace IDs in stage display order, used to keep single-group cache keys exact.
pub(crate) fn stage_ids_of(
    snapshots: &[Snapshot],
    handovers: &HashMap<String, Handover>,
) -> Vec<String> {
    let mut order: Vec<usize> = (0..snapshots.len()).collect();
    order.sort_by(|&a, &b| {
        (
            timestamp(&snapshots[a].item.updated).unwrap_or_default(),
            &snapshots[a].item.id,
        )
            .cmp(&(
                timestamp(&snapshots[b].item.updated).unwrap_or_default(),
                &snapshots[b].item.id,
            ))
    });
    let mut groups = BTreeMap::new();
    for index in order {
        let snapshot = &snapshots[index];
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
    let tier = |status: &str| match status {
        "running" | "review" => 0,
        _ => 1,
    };
    let mut groups: Vec<_> = groups.into_values().collect();
    groups.sort_by(|a, b| {
        (tier(a.status), Reverse(a.at), &a.id).cmp(&(tier(b.status), Reverse(b.at), &b.id))
    });
    groups.into_iter().take(5).map(|g| g.id).collect()
}

/// What the answer shows besides sessions: the isolated directories and each workspace's project.
pub(crate) struct Places<'a> {
    pub dirs: &'a [Dir],
    pub projects: &'a BTreeMap<String, Project>,
}

pub(crate) use origin::origin_cwds;

/// `limit` is the App's running limit (`settings.json`), which also caps the desks drawn.
pub(crate) fn respond(
    path: &str,
    mut snapshots: Vec<Snapshot>,
    data_exists: bool,
    limit: usize,
    handovers: &HashMap<String, Handover>,
    apps: &HashMap<String, &'static str>,
    places: &Places,
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
    let (mut groups, stage_ids) = unprojected_groups(&mut snapshots, handovers, apps, places);
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
        value["isolated"] = json!(group.isolated);
        let rooms = group.rooms();
        let messages = group.timeline(focus, complete);
        let omitted = (group.messages.len() + group.typing.len()).saturating_sub(messages.len());
        let referenced = referenced_desks(&rooms, &messages);
        let (desks, optimized) =
            group.desks(host.as_deref(), limit, false, OPTIMIZED_LISTED, &referenced);
        value["desks"] = json!(desks);
        value["optimized"] = optimized;
        value["rooms"] = json!(rooms);
        value["messages"] = json!(messages);
        value["boss_messages"] = json!(group.boss_messages());
        value["omitted"] = json!(omitted);
        value["threads"] = json!(group.threads());
        return Response::json(&value);
    }
    if path == "/app/stage" {
        panel_response(groups, &snapshots, &stage_ids, data_exists)
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
            Some(
                "antigravity"
                    | "claude"
                    | "codebuddy"
                    | "codex"
                    | "deepseek"
                    | "devin"
                    | "grok"
                    | "pi"
            )
        )
}

/// Where a workspace's running calls work.
fn running_cwds(snapshots: &[Snapshot], sources: &[usize]) -> Vec<String> {
    sources
        .iter()
        .flat_map(|&index| running_in(&snapshots[index].view["director"]))
        .collect()
}

/// Where one conversation's running calls work: a task's own directory, or a running seat's,
/// falling back to the meeting's for views saved before seats carried one.
fn running_in(director: &Value) -> Vec<String> {
    let mut found: Vec<String> = array(&director["round"], "seats")
        .filter(|seat| seat["status"] == "running")
        .filter_map(|seat| {
            array(director, "seats")
                .find(|s| s["id"] == seat["seat_id"])
                .and_then(|s| opt_text(s, "cwd"))
                .or_else(|| opt_text(director, "cwd"))
        })
        .collect();
    if found.is_empty() && director["round"]["status"] == "running" {
        found.extend(opt_text(director, "cwd"));
    }
    found.extend(
        array(director, "tasks")
            .filter(|task| task["status"] == "running")
            .filter_map(|task| opt_text(task, "cwd")),
    );
    found
}

/// The task calls of a workspace that created an isolated directory, with their desk and the
/// first line of their prompt.
fn isolated_builders(snapshots: &[Snapshot], sources: &[usize]) -> Vec<isolated::Builder> {
    let mut found = Vec::new();
    for &index in sources {
        let snapshot = &snapshots[index];
        for (number, task) in array(&snapshot.view["director"], "tasks").enumerate() {
            let (id, path) = (text(task, "isolation_id"), text(task, "isolation_path"));
            if id.is_empty() && path.is_empty() {
                continue;
            }
            let task_id = opt_text(task, "id").unwrap_or_else(|| number.to_string());
            let first = text(task, "prompt")
                .lines()
                .map(str::trim)
                .find(|l| !l.is_empty());
            found.push(isolated::Builder {
                id: id.to_owned(),
                path: path.to_owned(),
                desk: task_desk(&snapshot.item.id, task, &task_id),
                text: first.unwrap_or_default().chars().take(80).collect(),
            });
        }
    }
    found
}

/// The name a working directory gives a workspace before its project is known: its folder name,
/// or the repository's name for an agent worktree such as `<repo>/.claude/worktrees/<name>`.
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

fn unprojected_groups(
    snapshots: &mut [Snapshot],
    handovers: &HashMap<String, Handover>,
    apps: &HashMap<String, &'static str>,
    places: &Places,
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
        if let Some(handover) = handovers.get(&group.id) {
            group.handover = handover.clone();
            if group.first_title.is_none()
                && let Some(title) = &handover.title
            {
                group.title.clone_from(title);
            }
        }
        group.app = apps.get(&group.id).copied();
        group.project = places.projects.get(&group.id).cloned();
        let running = running_cwds(snapshots, &group.sources);
        let builders = isolated_builders(snapshots, &group.sources);
        group.isolated = isolated::classify(places.dirs, &group.id, &running, &builders);
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
            value["boss_messages"] = json!(g.boss_messages());
            let rooms = g.rooms();
            // Running desks and the most recent others; the panel's cards show at most that many.
            let referenced = referenced_desks(&rooms, &[]);
            let (desks, optimized) =
                g.desks(g.host.as_deref(), PANEL_IDLE_DESKS, true, 0, &referenced);
            value["desks"] = json!(desks);
            value["optimized"] = optimized;
            value["rooms"] = json!(rooms);
            value
        })
        .collect();
    let counts = snapshots
        .iter()
        .filter(|s| chat_group(&s.item).is_some_and(|id| stage_ids.contains(&id)))
        .map(crate::data::snapshot::call_counts)
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
            let mut projected_messages = false;
            let preview = match previews.remove(&g.id) {
                Some(preview) if preview.0 == key => preview,
                _ => {
                    g.project(snapshots, true);
                    projected_messages = true;
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
            if !projected_messages {
                g.project(snapshots, false);
            }
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
