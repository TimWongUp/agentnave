//! Read-only projections of Studio snapshots for the stage and manager chats.
use std::cmp::Reverse;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::data::{Item, Response, Snapshot, valid_id};

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
        Some("gemini") => "Gemini CLI",
        Some("opencode") => "OpenCode",
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

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as i64
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
        "codex" | "claude" | "codebuddy" | "grok" | "antigravity" | "pi" | "deepseek" => {
            label(Some(provider))
        }
        other => other,
    }
}

fn desk_label(model: &str, provider: &str, role: Option<&str>) -> String {
    let mut parts = vec![
        if model.is_empty() {
            "默认模型".to_owned()
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

fn handle(model: &str, provider: &str, role: Option<&str>) -> String {
    let chars: Vec<_> = model.chars().collect();
    let model = if chars.len() > 20 {
        format!(
            "{}…{}",
            chars[..10].iter().collect::<String>(),
            chars[chars.len() - 9..].iter().collect::<String>()
        )
    } else if model.is_empty() {
        "默认模型".to_owned()
    } else {
        model.to_owned()
    };
    let mut value = format!("@{model}·{}", cli_label(provider));
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
        Some("gemini") | Some("opencode") | Some("hermes") | None => None,
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

/// A desk seen in one workspace: one native session (a private thread or a show seat), with
/// the labels of its latest call and its history.
#[derive(Default)]
struct Desk {
    key: String,
    // The room a seat desk belongs to; None for a private thread.
    room: Option<String>,
    provider: String,
    model: String,
    role: Option<String>,
    labelled: Option<i64>,
    // Several desks with the same model, CLI and role are numbered by first appearance.
    ordinal: usize,
    first: Option<i64>,
    last: Option<i64>,
    latest: Option<(i64, String)>,
    active: bool,
    tasks: usize,
    running: usize,
    activities: Vec<Value>,
    reported: Option<(i64, String)>,
    // The latest declared access tier; the server records declarations without enforcing them.
    access: Option<(i64, String)>,
}

impl Desk {
    /// Model and role follow the desk's latest call; a resumed thread may relabel it.
    fn label(&mut self, at: i64, model: &str, role: Option<String>) {
        if self.labelled.is_none_or(|old| at >= old) {
            self.labelled = Some(at);
            self.model = model.to_owned();
            self.role = role;
        }
    }

    fn base(&self) -> (String, String, Option<String>) {
        (
            self.display_model().to_owned(),
            self.provider.clone(),
            self.role.clone(),
        )
    }

    fn declare(&mut self, at: i64, access: Option<String>) {
        if let Some(access) = access
            && self.access.as_ref().is_none_or(|(old_at, _)| at >= *old_at)
        {
            self.access = Some((at, access));
        }
    }

    fn seen(&mut self, at: i64) {
        self.first = Some(self.first.map_or(at, |first| first.min(at)));
        self.last = Some(self.last.map_or(at, |last| last.max(at)));
    }

    fn observe(&mut self, at: i64, status: &str) {
        self.seen(at);
        let status = member_status(status);
        if active(status) {
            self.active = true;
        } else if self.latest.as_ref().is_none_or(|(old, _)| at >= *old) {
            self.latest = Some((at, status.to_owned()));
        }
    }

    fn seat(&mut self, at: i64, status: &str, meeting: Option<(&str, &str)>) {
        self.observe(at, status);
        let status = member_status(status);
        if let Some((room, seat)) = meeting
            && active(status)
        {
            self.activities
                .push(json!({"kind": "meeting", "room": room, "seat": seat, "status": status}));
        }
    }

    fn reported(&mut self, at: i64, identity: &Value) {
        if let Some(model) = opt_text(identity, "reported_model")
            && self.reported.as_ref().is_none_or(|(old, _)| at >= *old)
        {
            self.reported = Some((at, model));
        }
    }

    fn display_model(&self) -> &str {
        if !self.model.is_empty() {
            &self.model
        } else {
            self.reported.as_ref().map_or("", |(_, model)| model)
        }
    }

    fn numbered(&self, name: String) -> String {
        if self.ordinal == 0 {
            name
        } else {
            format!("{name} #{}", self.ordinal)
        }
    }

    fn handle(&self) -> String {
        self.numbered(handle(
            self.display_model(),
            &self.provider,
            self.role.as_deref(),
        ))
    }

    fn status(&self) -> &'static str {
        if self.active {
            "running"
        } else {
            match &self.latest {
                Some((_, status)) if status == "pending" => "pending",
                Some((_, status)) if urgency(status) == 1 => "failed",
                _ => "idle",
            }
        }
    }

    fn value(&self, host: Option<&str>) -> Value {
        let status = self.status();
        let meeting = self
            .activities
            .iter()
            .rev()
            .find(|a| a["kind"] == "meeting")
            .map(|a| json!({"room": a["room"], "seat": a["seat"], "status": a["status"]}))
            .unwrap_or_else(
                || json!({"room": Value::Null, "seat": Value::Null, "status": Value::Null}),
            );
        json!({
            "key": self.key,
            "kind": if self.room.is_some() { "seat" } else { "task" },
            "room": self.room,
            "provider": self.provider,
            "model": self.model,
            "role": self.role,
            "access": self.access.as_ref().map(|(_, access)| access),
            "label": self.numbered(desk_label(self.display_model(), &self.provider, self.role.as_deref())),
            "display_model": self.display_model(),
            "model_source": if !self.model.is_empty() { "requested" } else if self.reported.is_some() { "native" } else { "unknown" },
            "handle": self.handle(), "activities": self.activities.iter().rev().collect::<Vec<_>>(),
            "first_seen": iso(self.first.unwrap_or_default()),
            "last_active": iso(self.last.unwrap_or_default()),
            "status": status,
            "private_allowed": host_provider(host) != Some(self.provider.as_str()),
            "private": {"tasks": self.tasks, "running": self.running},
            "meeting": meeting,
        })
    }
}

fn emphasis_end(text: &str, marker: &str) -> Option<usize> {
    let mut rest = text;
    while !rest.is_empty() {
        if rest.starts_with('`') {
            let size = rest.bytes().take_while(|b| *b == b'`').count();
            if let Some(end) = rest[size..].find(&rest[..size]) {
                rest = &rest[size + end + size..];
                continue;
            }
        }
        if rest.starts_with(marker) {
            return Some(text.len() - rest.len());
        }
        rest = &rest[rest.chars().next()?.len_utf8()..];
    }
    None
}

fn inline_text(mut rest: &str) -> String {
    let mut result = String::new();
    while let Some(first) = rest.chars().next() {
        if first == '`' {
            let size = rest.bytes().take_while(|b| *b == b'`').count();
            let marker = &rest[..size];
            if let Some(end) = rest[size..].find(marker) {
                result.push_str(&rest[size..size + end]);
                rest = &rest[size + end + size..];
                continue;
            }
        }
        let marker = if rest.starts_with("**") { "**" } else { "__" };
        if rest.starts_with(marker)
            && let Some(end) = emphasis_end(&rest[2..], marker).filter(|end| *end > 0)
        {
            let after = &rest[2 + end + 2..];
            // Underscores inside names are literal, rather than emphasis delimiters.
            let in_word = marker == "__"
                && (result.ends_with(char::is_alphanumeric)
                    || after.starts_with(char::is_alphanumeric));
            if !in_word {
                result.push_str(&inline_text(&rest[2..2 + end]));
                rest = after;
                continue;
            }
        }
        result.push(first);
        rest = &rest[first.len_utf8()..];
    }
    result
}

fn plain(line: &str) -> String {
    let mut line = line.trim();
    loop {
        let heading = line.bytes().take_while(|b| *b == b'#').count();
        let numbered = line.trim_start_matches(|c: char| c.is_ascii_digit());
        let next = if (1..=6).contains(&heading) && line[heading..].starts_with(char::is_whitespace)
        {
            Some(&line[heading..])
        } else {
            line.strip_prefix("> ")
                .or_else(|| line.strip_prefix("- "))
                .or_else(|| line.strip_prefix("+ "))
                .or_else(|| line.strip_prefix("* "))
                .or_else(|| {
                    if numbered.len() < line.len() {
                        numbered
                            .strip_prefix(". ")
                            .or_else(|| numbered.strip_prefix(") "))
                    } else {
                        None
                    }
                })
        };
        let Some(next) = next else {
            break;
        };
        line = next.trim_start();
    }
    inline_text(line).trim().chars().take(120).collect()
}

fn line(status: &str, output: &str, error: &str) -> String {
    if status == "review" {
        return "待审核：请在 boss 中决定".into();
    }
    let lines = || output.lines().map(plain).filter(|s| !s.is_empty());
    match status {
        "running" => lines().next_back().unwrap_or_default(),
        "succeeded" => lines().next().unwrap_or_default(),
        _ => {
            let error = error
                .lines()
                .map(plain)
                .find(|s| !s.is_empty())
                .unwrap_or_default();
            format!(
                "{}{}{}",
                failure(status),
                if error.is_empty() { "" } else { "：" },
                error
            )
            .chars()
            .take(120)
            .collect()
        }
    }
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

struct Group {
    id: String,
    host: Option<String>,
    updated: String,
    at: i64,
    title: String,
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

impl Group {
    fn new(id: String) -> Self {
        Self {
            color: color(&id),
            id,
            latest_end: None,
            host: None,
            updated: String::new(),
            at: i64::MIN,
            title: String::new(),
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

    /// Conversations saved before hosts were recorded have neither host nor manager session.
    fn manager_label(&self) -> &str {
        match self.host.as_deref() {
            None if self.id.starts_with("c-") => "未记录 boss",
            host => label(host),
        }
    }

    fn header(&self) -> Value {
        json!({"group": self.id, "host": self.host, "label": self.manager_label(),
            "project": self.project_name, "title": self.title,
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
    /// running or under review, plus the rest as the "optimized" list, most recent first.
    fn desks(&self, host: Option<&str>, limit: usize) -> (Vec<Value>, Value) {
        let mut desks: Vec<_> = self.desks.values().collect();
        desks.sort_by_key(|d| (!d.active, Reverse(d.last), &d.key));
        let busy = desks.iter().filter(|d| d.active).count();
        let rest = desks.split_off(limit.max(busy).min(desks.len()));
        // Drawn desks keep the office's own order: first seen, so a desk never jumps.
        desks.sort_by_key(|d| (d.first.unwrap_or_default(), &d.key));
        let shown = desks.into_iter().map(|desk| desk.value(host)).collect();
        let optimized = json!({
            "count": rest.len(),
            "desks": rest.iter().take(OPTIMIZED_LISTED).map(|desk| desk.value(host)).collect::<Vec<_>>(),
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
        desk.label(created, text(identity, "model"), desk_role(identity));
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
            self.title = self.project_name.clone().unwrap_or_default();
        }
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

    fn show(&mut self, snapshot: &Snapshot) {
        let item = &snapshot.item;
        let view = &snapshot.view["director"];
        let seats: BTreeMap<_, _> = array(view, "seats").map(|s| (text(s, "id"), s)).collect();
        let created = timestamp(text(view, "created"))
            .or_else(|| timestamp(&item.updated))
            .unwrap_or_default();
        let mut latest = BTreeMap::new();
        for (index, entry) in array(view, "messages").enumerate() {
            let kind = text(entry, "kind");
            let forwarded = kind == "forward";
            let seat = seats.get(text(entry, "speaker")).copied();
            let mut identity = seat.cloned().unwrap_or(Value::Null);
            if seat.is_some() {
                identity["seat"] = json!(text(entry, "speaker"));
            }
            if !text(entry, "reported_model").is_empty() {
                identity["reported_model"] = entry["reported_model"].clone();
            }
            let manager = matches!(kind, "task" | "host");
            let absent = kind == "absent";
            let time = event_time(text(entry, "time"), &item.updated);
            let status = if absent {
                let body = text(entry, "text");
                ["blocked", "cancelled", "interrupted"]
                    .into_iter()
                    .find(|s| body.contains(failure(s)) || body == *s)
                    .unwrap_or("failed")
            } else {
                "succeeded"
            };
            let desk = entry_desk(&item.id, entry, &seats);
            let message_id = opt_text(entry, "message_id");
            if !manager && !forwarded && seat.is_some() {
                let speaker = text(entry, "speaker");
                let seat_key = seat_desk(&item.id, speaker);
                self.member(
                    snapshot,
                    (&seat_key, &identity),
                    status,
                    text(entry, "text"),
                    "",
                    &time,
                );
                latest.insert(speaker, entry);
                self.desk_seat(
                    (&item.id, speaker),
                    &identity,
                    status,
                    created,
                    timestamp(&time).unwrap_or(created),
                    false,
                );
            }
            if !self.include_messages {
                continue;
            }
            let who = if forwarded {
                manager_speaker(entry, self.host.as_deref().unwrap_or(""), "boss 转发")
            } else if manager {
                manager_speaker(
                    entry,
                    self.host.as_deref().unwrap_or(""),
                    self.manager_label(),
                )
            } else if seat.is_some() {
                member_speaker(&identity)
            } else {
                speaker("system", "", "", "系统")
            };
            let mut msg = Self::message(
                format!("{}:entry:{index}", item.id),
                &time,
                &item.id,
                if forwarded {
                    "forward"
                } else if manager {
                    "prompt"
                } else if absent {
                    "notice"
                } else {
                    "reply"
                },
                who,
                if absent {
                    "本轮缺席"
                } else {
                    text(entry, "text")
                },
            );
            msg["round"] = entry["round"].clone();
            if kind == "task" {
                let round = array(view, "rounds")
                    .find(|r| r["number"] == entry["round"])
                    .or_else(|| {
                        (view["round"]["number"] == entry["round"]).then_some(&view["round"])
                    });
                if let Some(round) = round {
                    let answering: Vec<_> = array(round, "seats")
                        .map(|r| json!(text(r, "seat_id")))
                        .collect();
                    if answering.len() < seats.len() {
                        msg["answering"] = json!(answering);
                    }
                }
            }
            let forward = if forwarded {
                opt_text(entry, "forward_id").map(|message_id| {
                    json!({"message_id": message_id, "source": text(entry, "forward_source")})
                })
            } else {
                None
            };
            annotate(
                &mut msg,
                "public",
                room_thread(&item.id),
                desk.as_deref(),
                message_id.as_deref(),
                forward,
            );
            if manager {
                let recipients = if entry["recipients"].is_array() {
                    array(entry, "recipients")
                        .filter_map(|r| {
                            seats.get(text(r, "seat")).map(|seat| {
                                let mut identity = (*seat).clone();
                                identity["seat"] = r["seat"].clone();
                                recipient(&seat_desk(&item.id, text(r, "seat")), &identity)
                            })
                        })
                        .collect()
                } else {
                    seats
                        .iter()
                        .map(|(id, seat)| {
                            let mut identity = (*seat).clone();
                            identity["seat"] = json!(id);
                            recipient(&seat_desk(&item.id, id), &identity)
                        })
                        .collect()
                };
                route_to(&mut msg, recipients);
            } else {
                msg["status"] = json!(status);
                if seat.is_some() {
                    msg["focus"] = json!(format!("seat:{}:{}", item.id, text(entry, "speaker")));
                }
            }
            if kind == "answer" {
                msg["id"] = json!(format!(
                    "{}:round:{}:{}:result",
                    item.id,
                    entry["round"],
                    text(entry, "speaker")
                ));
            }
            self.messages.push(msg);
        }
        for entry in array(view, "instructions") {
            let id = text(entry, "seat_id");
            if let Some(seat) = seats.get(id)
                && self.include_messages
            {
                let mut identity = (*seat).clone();
                identity["seat"] = json!(id);
                let mut msg = Self::message(
                    format!("{}:instruction:{}:{id}", item.id, entry["round"]),
                    &event_time(text(entry, "time"), &item.updated),
                    &item.id,
                    "instruction",
                    manager_speaker(
                        entry,
                        self.host.as_deref().unwrap_or(""),
                        self.manager_label(),
                    ),
                    text(entry, "text"),
                );
                msg["round"] = entry["round"].clone();
                route_to(
                    &mut msg,
                    vec![recipient(&seat_desk(&item.id, id), &identity)],
                );
                annotate(
                    &mut msg,
                    "private",
                    room_thread(&item.id),
                    desk_of(&item.id, &identity).as_deref(),
                    None,
                    None,
                );
                self.messages.push(msg);
            }
        }
        for entry in array(view, "private_messages") {
            let id = text(entry, "seat_id");
            let Some(seat) = seats.get(id).copied() else {
                continue;
            };
            let time = event_time(text(entry, "time"), &item.updated);
            let mut identity = seat.clone();
            identity["seat"] = json!(id);
            if !text(entry, "reported_model").is_empty() {
                identity["reported_model"] = entry["reported_model"].clone();
            }
            self.member(
                snapshot,
                (&seat_desk(&item.id, id), &identity),
                "succeeded",
                text(entry, "text"),
                "",
                &time,
            );
            latest.insert(id, entry);
            self.desk_seat(
                (&item.id, id),
                &identity,
                "succeeded",
                created,
                timestamp(&time).unwrap_or(created),
                false,
            );
            if self.include_messages {
                let round = entry["round"].as_u64().unwrap_or_default();
                let mut msg = Self::message(
                    format!("{}:round:{round}:{id}:private", item.id),
                    &time,
                    &item.id,
                    "reply",
                    member_speaker(&identity),
                    text(entry, "text"),
                );
                msg["private"] = json!(true);
                msg["kept_private"] = json!(true);
                msg["round"] = entry["round"].clone();
                msg["status"] = json!("succeeded");
                msg["focus"] = json!(format!("seat:{}:{id}", item.id));
                annotate(
                    &mut msg,
                    "private",
                    room_thread(&item.id),
                    desk_of(&item.id, &identity).as_deref(),
                    None,
                    None,
                );
                self.messages.push(msg);
            }
        }
        for (id, seat) in &seats {
            let run = array(&view["round"], "seats").find(|r| text(r, "seat_id") == *id);
            if let Some(run) = run {
                let mut identity = (*seat).clone();
                identity["seat"] = json!(id);
                if !text(run, "reported_model").is_empty() {
                    identity["reported_model"] = run["reported_model"].clone();
                }
                let status = if run["review"] == "pending" {
                    "review"
                } else {
                    member_status(text(run, "status"))
                };
                let round = view["round"]["number"].as_u64().unwrap_or_default();
                let committed = array(view, "messages").rev().find(|e| {
                    text(e, "speaker") == *id
                        && e["round"].as_u64() == Some(round)
                        && matches!(text(e, "kind"), "answer" | "absent")
                });
                // Host messages change conversation activity, never the time of a committed answer.
                let time = if status == "running" {
                    event_time(
                        snapshot.live_updated.as_deref().unwrap_or(""),
                        &item.updated,
                    )
                } else {
                    event_time(
                        committed.map_or("", |entry| text(entry, "time")),
                        &item.updated,
                    )
                };
                self.member(
                    snapshot,
                    (&seat_desk(&item.id, id), &identity),
                    status,
                    text(run, "output"),
                    text(run, "error"),
                    &time,
                );
                self.desk_seat(
                    (&item.id, id),
                    &identity,
                    status,
                    created,
                    timestamp(&time).unwrap_or(created),
                    true,
                );
                if self.include_messages
                    && (status == "review"
                        || (committed.is_none()
                            && matches!(
                                status,
                                "failed" | "blocked" | "cancelled" | "interrupted" | "succeeded"
                            )
                            // Records before v0.35 call an answer kept private "discard".
                            && !matches!(run["review"].as_str(), Some("private" | "discard"))))
                {
                    let body = if matches!(status, "review" | "succeeded") {
                        text(run, "output").to_owned()
                    } else {
                        failure(status).into()
                    };
                    let who = member_speaker(&identity);
                    let mut msg = Self::message(
                        format!("{}:round:{round}:{id}:result", item.id),
                        &time,
                        &item.id,
                        if matches!(status, "review" | "succeeded") {
                            "reply"
                        } else {
                            "notice"
                        },
                        who,
                        &body,
                    );
                    msg["status"] = json!(status);
                    msg["round"] = json!(round);
                    msg["detail"] = json!(text(run, "error"));
                    msg["focus"] = json!(format!("seat:{}:{id}", item.id));
                    annotate(
                        &mut msg,
                        "private",
                        room_thread(&item.id),
                        desk_of(&item.id, &identity).as_deref(),
                        None,
                        None,
                    );
                    self.messages.push(msg);
                }
            } else if !latest.contains_key(id) {
                self.member(
                    snapshot,
                    (&seat_desk(&item.id, id), seat),
                    "pending",
                    "",
                    "",
                    &event_time("", &item.updated),
                );
                self.desk_seat((&item.id, id), seat, "pending", created, created, false);
            }
        }
        let room_seats: Vec<Value> = seats
            .values()
            .filter(|seat| !text(seat, "provider").is_empty())
            .map(
                |seat| json!({"seat": text(seat, "id"), "desk": seat_desk(&item.id, text(seat, "id"))}),
            )
            .collect();
        self.rooms.push((
            created,
            item.id.clone(),
            json!({
                "id": item.id,
                "title": item.title,
                "status": conversation_status(snapshot),
                "seats": room_seats,
                "pending_review": array(&view["round"], "seats").filter(|run| run["review"] == "pending").count(),
                "round": view["round"].as_object().map(|r| json!({"number": r.get("number"), "order": r.get("order"), "status": r.get("status")})),
                "board": array(view, "messages").filter(|entry| text(entry, "kind") != "absent").count(),
            }),
        ));
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
type Preview = (Vec<SourceKey>, Vec<Arc<Value>>, String);
static PREVIEWS: LazyLock<Mutex<HashMap<String, Preview>>> = LazyLock::new(Mutex::default);

/// At most this many undrawn desks are listed; the count still covers all of them.
const OPTIMIZED_LISTED: usize = 200;

/// `limit` is the App's running limit (`settings.json`), which also caps the desks drawn.
pub(crate) fn respond(
    path: &str,
    mut snapshots: Vec<Snapshot>,
    data_exists: bool,
    limit: usize,
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
    let mut groups: Vec<_> = groups.into_values().collect();
    for group in &mut groups {
        group.project(&snapshots, false);
    }
    groups.sort_by(|a, b| {
        (!active(a.status), Reverse(a.at), &a.id).cmp(&(!active(b.status), Reverse(b.at), &b.id))
    });
    let cutoff = now() - 2 * 3600 * 1_000_000;
    let stage_ids: Vec<_> = groups
        .iter()
        .filter(|g| active(g.status) || g.at >= cutoff)
        .take(6)
        .map(|g| g.id.clone())
        .collect();
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
        let (desks, optimized) = group.desks(host.as_deref(), limit);
        value["desks"] = json!(desks);
        value["optimized"] = optimized;
        value["rooms"] = json!(group.rooms());
        value["messages"] = json!(group.timeline(focus));
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
                let (desks, optimized) = g.desks(g.host.as_deref(), limit);
                value["desks"] = json!(desks);
                // The panel only shows how many; the office lists them.
                value["optimized"] = json!({"count": optimized["count"]});
                value["rooms"] = json!(g.rooms());
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
                let key: Vec<_> = g
                    .sources
                    .iter()
                    .map(|&i| source_key(&snapshots[i]))
                    .collect();
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

    use super::handle;

    #[test]
    fn handles_shorten_only_the_model_middle_by_unicode_characters() {
        assert_eq!(
            handle("backend/abcdefghijklmno尾部模型", "pi", Some("review")),
            "@backend/ab…klmno尾部模型·Pi·review"
        );
    }
}
