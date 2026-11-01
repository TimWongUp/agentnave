//! Read-only projections of Studio snapshots for the stage and manager chats.
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, LazyLock, Mutex};

use serde_json::{Value, json};

use crate::data::{Item, Response, Snapshot, valid_id};

mod desk;
mod show;
mod summary;

use desk::Desk;
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

fn cli_label(provider: &str) -> &str {
    match provider {
        "codex" | "claude" | "codebuddy" | "grok" | "antigravity" | "pi" | "deepseek" | "zcode" => {
            label(Some(provider))
        }
        other => other,
    }
}

fn desk_label(model: &str, provider: &str, role: Option<&str>) -> String {
    let mut parts = vec![
        if model.is_empty() {
            "CLI 默认模型".to_owned()
        } else {
            model.to_owned()
        },
        cli_label(provider).to_owned(),
    ];
    if let Some(role) = role {
        parts.push(role.to_owned());
    }
    parts.join(" · ")
}

fn model_with_effort(model: &str, effort: Option<&str>) -> String {
    match effort.filter(|effort| !effort.is_empty()) {
        Some(effort) if model.is_empty() => format!("CLI 默认模型 · {effort}"),
        Some(effort) => format!("{model} · {effort}"),
        None => model.to_owned(),
    }
}

fn handle(model: &str, effort: Option<&str>, provider: &str, role: Option<&str>) -> String {
    let chars: Vec<_> = model.chars().collect();
    let model = if chars.len() > 20 {
        format!(
            "{}…{}",
            chars[..10].iter().collect::<String>(),
            chars[chars.len() - 9..].iter().collect::<String>()
        )
    } else if model.is_empty() {
        "CLI 默认模型".to_owned()
    } else {
        model.to_owned()
    };
    let mut value = format!(
        "@{}·{}",
        model_with_effort(&model, effort),
        cli_label(provider)
    );
    if let Some(role) = role {
        value.push('·');
        value.push_str(role);
    }
    value
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
    let model = model(identity);
    let mut who = speaker("member", provider, model, label(Some(provider)));
    who["effort"] = identity["effort"].clone();
    who["model_source"] = json!(if !text(identity, "reported_model").is_empty() {
        "native"
    } else if !model.is_empty() {
        "requested"
    } else {
        "unknown"
    });
    who
}

fn manager_speaker(identity: &Value, provider: &str, name: &str) -> Value {
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
    who
}

fn recipient(desk: &str, identity: &Value) -> Value {
    let provider = text(identity, "provider");
    let model = text(identity, "model");
    json!({"key": desk, "provider": provider, "model": model,
        "label": label(Some(provider)), "seat": text(identity, "seat")})
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
    message["to"] = json!(
        recipients
            .iter()
            .map(|r| json!({"key": r["key"], "handle": r["handle"]}))
            .collect::<Vec<_>>()
    );
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

impl Group {
    fn new(id: String) -> Self {
        Self {
            color: color(&id),
            id,
            latest_end: None,
            host: None,
            handover: Handover::default(),
            app: None,
            updated: String::new(),
            at: i64::MIN,
            title: String::new(),
            first_title: None,
            first_id: None,
            project_name: None,
            live: false,
            status: "idle",
            conversations: Vec::new(),
            members: BTreeMap::new(),
            desks: BTreeMap::new(),
            rooms: Vec::new(),
            messages: Vec::new(),
            typing: BTreeMap::new(),
            sources: Vec::new(),
            include_messages: false,
        }
    }

    /// A workspace a boss opened by handing over: only that boss, no desks or conversations yet.
    fn only_boss(id: String, handover: &Handover) -> Self {
        let mut group = Self::new(id);
        group.host.clone_from(&handover.host);
        group.title = handover.title.clone().unwrap_or_default();
        group.project_name = handover.cwd.as_deref().and_then(project_name);
        if let Some(released) = &handover.released {
            group.updated.clone_from(released);
            group.at = timestamp(released).unwrap_or_default();
        }
        group
    }

    /// Conversations saved before hosts were recorded have neither host nor manager session.
    fn manager_label(&self) -> &str {
        match self.host.as_deref() {
            None if self.id.starts_with("c-") => "未记录 boss",
            host => boss_name(host, self.app).1.unwrap_or(label(host)),
        }
    }

    fn header(&self) -> Value {
        let (host, name) = match &self.handover.host {
            Some(host) if self.handover.changed => (Some(host.as_str()), label(Some(host))),
            _ => (self.host.as_deref(), self.manager_label()),
        };
        let (app, named) = boss_name(host, self.app);
        let previous = self
            .handover
            .previous_host
            .as_deref()
            .map(|host| label(Some(host)))
            .unwrap_or("这个工作区");
        let label = named.unwrap_or(name);
        let handover = if self.handover.pending {
            Some("pending")
        } else {
            self.handover.changed.then_some("changed")
        };
        let took_over = (handover == Some("changed")).then(|| format!("{label} 接手了 {previous}"));
        json!({"group": self.id, "host": host, "app": app, "label": label, "handover": handover,
            "took_over": took_over, "project": self.project_name, "title": self.title,
            "title_id": self.first_title.as_ref().map(|(_, _, id)| id)
                .or(self.first_id.as_ref().map(|(_, id)| id)),
            "color": self.color, "live": self.live, "status": self.status, "updated": self.updated})
    }

    fn members(&self) -> Vec<Value> {
        let mut members: Vec<_> = self.members.values().collect();
        members.sort_by_key(|m| {
            (
                !active(text(&m.value, "status")),
                Reverse(m.at),
                text(&m.value, "key"),
            )
        });
        members.into_iter().map(|m| m.value.clone()).collect()
    }

    fn timeline(&self, focus: Option<&str>) -> Vec<Value> {
        let mut messages: Vec<_> = self.messages.iter().collect();
        messages.sort_by_key(|m| timestamp(text(m, "time")).unwrap_or_default());
        let mut typing: Vec<_> = self.typing.values().map(|(_, message)| message).collect();
        typing.sort_by_key(|m| {
            (
                timestamp(text(m, "time")).unwrap_or_default(),
                text(m, "id"),
            )
        });
        messages.extend(typing);
        let skip = messages.len().saturating_sub(300);
        messages
            .into_iter()
            .enumerate()
            .filter(|(index, message)| {
                *index >= skip || focus.is_some_and(|id| text(message, "conversation") == id)
            })
            .map(|(_, message)| message.clone())
            .collect()
    }

    /// The desks an office draws, at most `limit` (the App's running limit) unless more are
    /// running or under review, plus the rest as the "optimized" list, most recent first. The
    /// list holds the `listed` most recent and every one the payload refers to, so seats and
    /// messages always find their desk.
    fn desks(
        &self,
        host: Option<&str>,
        limit: usize,
        listed: usize,
        referenced: &BTreeSet<String>,
    ) -> (Vec<Value>, Value) {
        let mut desks: Vec<_> = self.desks.values().collect();
        desks.sort_by_key(|d| (!d.active, Reverse(d.last), &d.key));
        let busy = desks.iter().filter(|d| d.active).count();
        let rest = desks.split_off(limit.max(busy).min(desks.len()));
        // Drawn desks put the newest first; the others keep their order as new ones arrive.
        desks.sort_by_key(|d| (Reverse(d.first.unwrap_or_default()), &d.key));
        let shown = desks.into_iter().map(|desk| desk.value(host)).collect();
        let optimized = json!({
            "count": rest.len(),
            "desks": rest
                .iter()
                .enumerate()
                .filter(|(index, desk)| *index < listed || referenced.contains(&desk.key))
                .map(|(_, desk)| desk.value(host))
                .collect::<Vec<_>>(),
        });
        (shown, optimized)
    }

    /// Desks sharing model, CLI and role are numbered in the order they first appeared.
    fn number_desks(&mut self) {
        let mut same = BTreeMap::<_, Vec<(i64, String)>>::new();
        for desk in self.desks.values() {
            same.entry(desk.base())
                .or_default()
                .push((desk.first.unwrap_or_default(), desk.key.clone()));
        }
        for mut keys in same.into_values().filter(|keys| keys.len() > 1) {
            keys.sort();
            for (index, (_, key)) in keys.iter().enumerate() {
                if let Some(desk) = self.desks.get_mut(key) {
                    desk.ordinal = index + 1;
                }
            }
        }
    }

    fn rooms(&self) -> Vec<Value> {
        let mut rooms: Vec<_> = self.rooms.iter().collect();
        rooms.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
        rooms.into_iter().map(|(_, _, room)| room.clone()).collect()
    }

    /// Record one seat observation; `created` anchors first_seen so ordering stays stable.
    fn desk_seat(
        &mut self,
        (room, seat): (&str, &str),
        identity: &Value,
        status: &str,
        created: i64,
        at: i64,
        meeting: bool,
    ) {
        let provider = text(identity, "provider");
        if provider.is_empty() {
            return;
        }
        let key = seat_desk(room, seat);
        let desk = self.desks.entry(key.clone()).or_insert_with(|| Desk {
            key,
            room: Some(room.to_owned()),
            provider: provider.to_owned(),
            ..Desk::default()
        });
        desk.label(
            created,
            text(identity, "model"),
            opt_text(identity, "effort"),
            desk_role(identity),
        );
        desk.seen(created);
        desk.declare(created, opt_text(identity, "access"));
        desk.seat(at, status, meeting.then_some((room, seat)));
        desk.reported(at, identity);
    }

    fn message(
        id: String,
        time: &str,
        conversation: &str,
        kind: &str,
        who: Value,
        body: &str,
    ) -> Value {
        json!({"id": id, "time": time, "conversation": conversation, "kind": kind,
            "speaker": who, "to": [], "text": body, "status": "", "detail": "",
            "message_id": Value::Null, "visibility": "private", "thread": Value::Null,
            "desk": Value::Null, "forward": Value::Null, "reply_to": Value::Null})
    }

    fn member(
        &mut self,
        snapshot: &Snapshot,
        (desk, identity): (&str, &Value),
        status: &str,
        output: &str,
        error: &str,
        time: &str,
    ) {
        let provider = text(identity, "provider");
        if provider.is_empty() {
            return;
        }
        let model = model(identity);
        let key = format!("{provider}/{model}");
        let at = timestamp(time).unwrap_or_default();
        let status = member_status(status);
        if !active(status)
            && status != "pending"
            && self
                .latest_end
                .as_ref()
                .is_none_or(|(old_at, _)| at >= *old_at)
        {
            self.latest_end = Some((at, status.to_owned()));
        }
        let who = member_speaker(identity);
        let typing = if self.include_messages && status == "running" {
            let tail: Vec<_> = output
                .lines()
                .rev()
                .filter(|s| !s.trim().is_empty())
                .take(4)
                .collect();
            let tail = tail.into_iter().rev().collect::<Vec<_>>().join("\n");
            let mut message = Self::message(
                key.clone(),
                &event_time(
                    snapshot.live_updated.as_deref().unwrap_or(""),
                    &snapshot.item.updated,
                ),
                &snapshot.item.id,
                "typing",
                who,
                &tail,
            );
            for key in ["doing", "output_at"] {
                if let Some(value) = identity[key].as_str() {
                    message[key] = json!(value);
                }
            }
            if !text(identity, "seat").is_empty() {
                message["focus"] = json!(format!(
                    "seat:{}:{}",
                    snapshot.item.id,
                    text(identity, "seat")
                ));
            }
            let seat = text(identity, "seat");
            let thread = if seat.is_empty() {
                desk_thread(desk)
            } else {
                room_thread(&snapshot.item.id)
            };
            let message_id = if seat.is_empty() {
                opt_text(identity, "message_id")
            } else {
                None
            };
            annotate(
                &mut message,
                "private",
                thread,
                Some(desk),
                message_id.as_deref(),
                None,
            );
            message["round"] = snapshot.view["director"]["round"]["number"].clone();
            if seat.is_empty() {
                message["reply_to"] = json!(format!(
                    "{}:task:{}:prompt",
                    snapshot.item.id,
                    text(identity, "id")
                ));
            }
            // One bubble per running thread: desks sharing a provider/model, or one desk's private
            // and meeting work running together, must not overwrite each other.
            let typing_key = format!("typing:{desk}:{}:{seat}", snapshot.item.id);
            message["id"] = json!(typing_key);
            Some((typing_key, message))
        } else {
            None
        };
        if let Some((typing_key, typing)) = typing
            && self
                .typing
                .get(&typing_key)
                .is_none_or(|(old_at, _)| at >= *old_at)
        {
            self.typing.insert(typing_key, (at, typing));
        }
        let member = Member {
            at,
            value: json!({"key": key, "provider": provider, "model": model, "label": label(Some(provider)),
            "status": status, "line": line(status, output, error), "updated": time, "conversation": snapshot.item.id}),
        };
        if self.members.get(&key).is_none_or(|old| at >= old.at) {
            self.members.insert(key, member);
        }
    }

    fn add(&mut self, snapshot: &Snapshot, index: usize) {
        let item = &snapshot.item;
        let at = timestamp(&item.updated).unwrap_or_default();
        if at >= self.at {
            self.at = at;
            self.updated.clone_from(&item.updated);
            self.host.clone_from(&item.host);
            let director = &snapshot.view["director"];
            let cwd = opt_text(director, "cwd").or_else(|| {
                array(director, "tasks")
                    .filter_map(|t| opt_text(t, "cwd"))
                    .next_back()
            });
            if let Some(name) = cwd.as_deref().and_then(project_name) {
                self.project_name = Some(name);
            }
        }
        // The first dispatch names the whole chat and never changes; later titles name only
        // their own chat or meeting.
        let created = timestamp(text(&snapshot.view["director"], "created")).unwrap_or(at);
        if self.first_id.as_ref().is_none_or(|(c, _)| created < *c) {
            self.first_id = Some((created, item.id.clone()));
        }
        if meaningful_title(&item.title)
            && self
                .first_title
                .as_ref()
                .is_none_or(|(c, _, _)| created < *c)
        {
            self.first_title = Some((created, item.title.clone(), item.id.clone()));
        }
        self.title = self
            .first_title
            .as_ref()
            .map(|(_, t, _)| t.clone())
            .or_else(|| self.project_name.clone())
            .unwrap_or_default();
        self.live |= snapshot.live;
        let status = conversation_status(snapshot);
        if urgency(status) > urgency(self.status) {
            self.status = status;
        }
        self.sources.push(index);
        self.conversations.push(
            json!({"id": item.id, "title": item.title, "mode": item.mode, "status": item.status}),
        );
    }

    fn project(&mut self, snapshots: &[Snapshot], include_messages: bool) {
        self.include_messages = include_messages;
        self.latest_end = None;
        self.desks.clear();
        self.members.clear();
        self.messages.clear();
        self.typing.clear();
        self.rooms.clear();
        for index in 0..self.sources.len() {
            let snapshot = &snapshots[self.sources[index]];
            if snapshot.item.mode == "task" {
                self.tasks(snapshot);
            } else {
                self.show(snapshot);
            }
        }
        self.number_desks();
        self.finish_messages();
        if include_messages
            && (self.handover.pending || self.handover.changed)
            && let Some(released) = self.handover.released.as_deref()
        {
            // Only the latest handover is stored. Its timestamp is the release time;
            // the database does not record when the next boss took over.
            let from = if self.handover.pending {
                self.handover.host.as_deref().or(self.host.as_deref())
            } else {
                self.handover.previous_host.as_deref()
            };
            let text = if self.handover.pending {
                format!("{} 交出工作区，等待接手", label(from))
            } else {
                match from {
                    Some(from) => format!(
                        "{} 交出工作区，{} 已接手",
                        label(Some(from)),
                        label(self.handover.host.as_deref())
                    ),
                    None => format!(
                        "工作区已交出，{} 已接手",
                        label(self.handover.host.as_deref())
                    ),
                }
            };
            let mut message = Self::message(
                format!("{}:handover:{released}", self.id),
                released,
                "",
                "handover",
                json!({"role": "system", "label": "工作区交接"}),
                &text,
            );
            message["detail"] = json!(self.handover.note);
            self.messages.push(message);
        }
        if !active(self.status) {
            self.status = if self
                .latest_end
                .as_ref()
                .is_some_and(|(_, status)| urgency(status) == 1)
            {
                "failed"
            } else {
                "idle"
            };
        }
    }

    fn finish_messages(&mut self) {
        let handles: BTreeMap<_, _> = self
            .desks
            .iter()
            .map(|(key, desk)| (key.clone(), desk.handle()))
            .collect();
        let mut seat_handles = BTreeMap::new();
        for (_, room_id, room) in &mut self.rooms {
            for seat in room["seats"].as_array_mut().into_iter().flatten() {
                // Each seat is its own desk; numbering already tells alike seats apart.
                let handle = handles.get(text(seat, "desk")).cloned().unwrap_or_default();
                seat_handles.insert(
                    (room_id.clone(), text(seat, "seat").to_owned()),
                    handle.clone(),
                );
                seat["handle"] = json!(handle);
            }
        }
        // The round's task comes first; a host remark posted after the round shares its number
        // and must not take over as the prompt the answers reply to.
        let mut prompts = BTreeMap::new();
        for m in self
            .messages
            .iter()
            .filter(|m| m["kind"] == "prompt" && m["thread"]["kind"] == "room")
        {
            prompts
                .entry((text(m, "conversation").to_owned(), m["round"].to_string()))
                .or_insert_with(|| m["id"].clone());
        }
        for msg in self
            .messages
            .iter_mut()
            .chain(self.typing.values_mut().map(|(_, m)| m))
        {
            let conversation = text(msg, "conversation").to_owned();
            let key = text(msg, "desk").to_owned();
            let seat = text(msg, "focus").rsplit(':').next().unwrap_or_default();
            let who = if msg["speaker"]["role"] == "manager" {
                "@boss".to_owned()
            } else {
                seat_handles
                    .get(&(conversation.clone(), seat.to_owned()))
                    .or_else(|| handles.get(&key))
                    .cloned()
                    .unwrap_or_default()
            };
            msg["speaker"]["handle"] = json!(who);
            for target in msg["recipients"].as_array_mut().into_iter().flatten() {
                target["handle"] = json!(
                    seat_handles
                        .get(&(conversation.clone(), text(target, "seat").to_owned()))
                        .or_else(|| handles.get(text(target, "key")))
                        .cloned()
                        .unwrap_or_default()
                );
            }
            if msg["kind"] == "instruction"
                || (msg["kind"] == "prompt" && msg["thread"]["kind"] == "desk")
            {
                msg["to"] = json!(
                    array(msg, "recipients")
                        .map(|r| json!({"key": r["key"], "handle": r["handle"]}))
                        .collect::<Vec<_>>()
                );
            } else if msg["visibility"] == "public" && msg["thread"]["kind"] == "room" {
                msg["to"] = json!([{"key": "@all", "handle": "@all"}]);
            } else {
                msg["to"] = json!([{"key": "@boss", "handle": "@boss"}]);
            }
            if let Some(answering) = msg.get_mut("answering").and_then(Value::as_array_mut) {
                for id in answering {
                    *id = json!(
                        seat_handles
                            .get(&(
                                conversation.clone(),
                                id.as_str().unwrap_or_default().to_owned()
                            ))
                            .cloned()
                            .unwrap_or_default()
                    );
                }
            }
            if msg["speaker"]["role"] != "manager" {
                if msg["thread"]["kind"] == "room" {
                    msg["reply_to"] = prompts
                        .get(&(conversation, msg["round"].to_string()))
                        .cloned()
                        .unwrap_or(Value::Null);
                } else if msg["kind"] != "typing"
                    && let Some((prefix, _)) = text(msg, "id").rsplit_once(':')
                {
                    msg["reply_to"] = json!(format!("{prefix}:prompt"));
                }
            }
        }
    }

    fn omitted(&self, focus: Option<&str>) -> usize {
        (self.messages.len() + self.typing.len()).saturating_sub(self.timeline(focus).len())
    }

    fn threads(&self) -> Vec<Value> {
        let mut threads = BTreeMap::<String, Value>::new();
        for msg in self
            .messages
            .iter()
            .chain(self.typing.values().map(|(_, m)| m))
        {
            let thread = &msg["thread"];
            if thread.is_null() {
                continue;
            }
            let key = thread.to_string();
            let entry = threads.entry(key).or_insert_with(|| {
                let mut value = thread.clone();
                value["count"] = json!(0);
                value["last"] = msg["time"].clone();
                if thread["kind"] == "desk" {
                    value["handle"] = self
                        .desks
                        .get(text(thread, "key"))
                        .map_or(Value::Null, |d| json!(d.handle()));
                } else {
                    value["title"] = self
                        .rooms
                        .iter()
                        .find(|(_, id, _)| id == text(thread, "id"))
                        .map_or(Value::Null, |(_, _, r)| r["title"].clone());
                }
                value
            });
            entry["count"] = json!(entry["count"].as_u64().unwrap_or_default() + 1);
            if timestamp(text(msg, "time")) > timestamp(text(entry, "last")) {
                entry["last"] = msg["time"].clone();
            }
        }
        let mut threads: Vec<_> = threads.into_values().collect();
        threads.sort_by_key(|t| Reverse(timestamp(text(t, "last"))));
        threads
    }

    fn tasks(&mut self, snapshot: &Snapshot) {
        let item = &snapshot.item;
        for (index, task) in array(&snapshot.view["director"], "tasks").enumerate() {
            let task_id = task["id"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| index.to_string());
            let id = format!("{}:task:{task_id}", item.id);
            let raw_start = text(task, "time");
            let start = event_time(raw_start, &item.updated);
            let time = timestamp(raw_start)
                .map(|start| {
                    iso(start.saturating_add(
                        task["elapsed_ms"]
                            .as_i64()
                            .unwrap_or(0)
                            .saturating_mul(1000),
                    ))
                })
                .unwrap_or_else(|| start.clone());
            let status = member_status(text(task, "status"));
            let provider = text(task, "provider");
            let desk = task_desk(&item.id, task, &task_id);
            let desk_ref = (!provider.is_empty()).then_some(desk.as_str());
            let message_id = opt_text(task, "message_id");
            if !provider.is_empty() {
                let entry = self.desks.entry(desk.clone()).or_insert_with(|| Desk {
                    key: desk.clone(),
                    provider: provider.to_owned(),
                    ..Desk::default()
                });
                entry.label(
                    timestamp(raw_start).unwrap_or_default(),
                    text(task, "model"),
                    opt_text(task, "effort"),
                    desk_role(task),
                );
                entry.seen(timestamp(raw_start).unwrap_or_default());
                entry.declare(
                    timestamp(raw_start).unwrap_or_default(),
                    opt_text(task, "access"),
                );
                entry.observe(timestamp(&time).unwrap_or_default(), status);
                entry.reported(timestamp(&time).unwrap_or_default(), task);
                if active(status) {
                    entry.activities.push(
                        json!({"kind": "private", "status": status, "conversation": item.id}),
                    );
                }
                entry.tasks += 1;
                if desk.starts_with("thread:") {
                    entry.aliases.push(format!("task:{}:{task_id}", item.id));
                }
                if status == "running" {
                    entry.running += 1;
                }
            }
            if self.include_messages
                && let Some(prompt) = task["prompt"].as_str()
            {
                let mut msg = Self::message(
                    format!("{id}:prompt"),
                    &start,
                    &item.id,
                    "prompt",
                    manager_speaker(
                        task,
                        self.host.as_deref().unwrap_or(""),
                        self.manager_label(),
                    ),
                    prompt,
                );
                route_to(&mut msg, vec![recipient(&desk, task)]);
                annotate(
                    &mut msg,
                    "private",
                    desk_thread(&desk),
                    desk_ref,
                    message_id.as_deref(),
                    None,
                );
                self.messages.push(msg);
            }
            self.member(
                snapshot,
                (&desk, task),
                status,
                text(task, "output"),
                text(task, "error"),
                &time,
            );
            if self.include_messages && status != "running" && status != "pending" {
                let who = member_speaker(task);
                // Failed calls can still contain a useful native answer; keep it beside the
                // failure notice now that chat is the App's only history reader.
                if !matches!(status, "succeeded" | "review") && !text(task, "output").is_empty() {
                    let mut output = Self::message(
                        format!("{id}:output"),
                        &time,
                        &item.id,
                        "reply",
                        who.clone(),
                        text(task, "output"),
                    );
                    output["status"] = json!(status);
                    annotate(
                        &mut output,
                        "private",
                        desk_thread(&desk),
                        desk_ref,
                        message_id.as_deref(),
                        None,
                    );
                    self.messages.push(output);
                }
                let body = if status == "succeeded" {
                    text(task, "output").to_owned()
                } else if status == "review" {
                    "待审核，请在 boss 中决定".into()
                } else {
                    failure(status).into()
                };
                let mut msg = Self::message(
                    format!("{id}:result"),
                    &time,
                    &item.id,
                    if status == "succeeded" {
                        "reply"
                    } else {
                        "notice"
                    },
                    who,
                    &body,
                );
                msg["status"] = json!(status);
                msg["detail"] = json!(text(task, "error"));
                annotate(
                    &mut msg,
                    "private",
                    desk_thread(&desk),
                    desk_ref,
                    message_id.as_deref(),
                    None,
                );
                self.messages.push(msg);
            }
        }
    }
}

/// Only true host-chat identities aggregate; old connection UUIDs cannot identify a chat.
pub(crate) fn chat_group(item: &Item) -> Option<String> {
    let session = item.manager_session.as_deref().filter(|id| {
        id.strip_prefix("s-")
            .is_some_and(|key| key.len() == 32 && key.bytes().all(|b| b.is_ascii_hexdigit()))
    });
    let id = session
        .map(str::to_owned)
        .unwrap_or_else(|| format!("c-{}", item.id));
    valid_id(&id).then_some(id)
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
        group.project(&snapshots, false);
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
    // Work going on first, then unhandled failures, then the most recent; never empty while history exists.
    let tier = |status: &str| match status {
        "running" | "review" => 0,
        "failed" => 1,
        _ => 2,
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
        let messages = group.timeline(focus);
        let referenced = referenced_desks(&rooms, &messages);
        let (desks, optimized) = group.desks(host.as_deref(), limit, OPTIMIZED_LISTED, &referenced);
        value["desks"] = json!(desks);
        value["optimized"] = optimized;
        value["rooms"] = json!(rooms);
        value["messages"] = json!(messages);
        value["omitted"] = json!(group.omitted(focus));
        value["threads"] = json!(group.threads());
        return Response::json(&value);
    }
    if path == "/app/stage" {
        let managers: Vec<_> = groups
            .into_iter()
            .filter(|g| stage_ids.contains(&g.id))
            .map(|mut g| {
                g.project(&snapshots, true);
                let mut value = g.header();
                value["threads"] = json!(g.threads());
                value["omitted"] = json!(g.omitted(None));
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
    } else {
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
                        g.project(&snapshots, true);
                        let text = g
                            .timeline(None)
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
mod tests {
    #[test]
    fn custom_titles_are_not_confused_with_provider_defaults() {
        assert!(super::meaningful_title("review 任务"));
        assert!(!super::meaningful_title("codex 任务"));
        assert!(!super::meaningful_title(" "));
    }

    #[test]
    fn project_name_skips_agent_worktrees() {
        assert_eq!(
            super::project_name("/Users/a/Code/app/").as_deref(),
            Some("app")
        );
        assert_eq!(
            super::project_name("/Users/a/Code/app/.claude/worktrees/fix-1").as_deref(),
            Some("app")
        );
        assert_eq!(
            super::project_name("/Users/a/.codex/worktrees/ab12/app").as_deref(),
            Some("app")
        );
        assert_eq!(super::project_name("/"), None);
    }

    use super::{desk_label, handle, model_with_effort};

    #[test]
    fn handles_shorten_only_the_model_middle_by_unicode_characters() {
        assert_eq!(
            handle(
                "backend/abcdefghijklmno尾部模型",
                None,
                "pi",
                Some("review")
            ),
            "@backend/ab…klmno尾部模型·Pi·review"
        );
        assert_eq!(
            handle(
                "backend/abcdefghijklmno尾部模型",
                Some("high"),
                "pi",
                Some("review")
            ),
            "@backend/ab…klmno尾部模型 · high·Pi·review"
        );
        assert_eq!(
            desk_label(&model_with_effort("", Some("high")), "codex", None),
            "CLI 默认模型 · high · Codex"
        );
    }
}
