//! Read-only projections of Studio snapshots for the stage and manager chats.
use std::cmp::Reverse;
use std::collections::BTreeMap;
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
        Some("gemini") => "Gemini CLI",
        Some("opencode") => "OpenCode",
        Some("hermes") => "Hermes",
        _ => "未知主控",
    }
}

fn color(group: &str) -> u64 {
    // FNV-1a, independent of process seeds and platform word size.
    group.bytes().fold(0xcbf29ce484222325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    }) % 8
}

/// Studio emits RFC3339 with microseconds. Compare instants, including offsets, not strings.
fn timestamp(time: &str) -> Option<i64> {
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

fn iso(time: i64) -> String {
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
        return "待审核：请在主控中决定".into();
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

fn recipient(identity: &Value) -> Value {
    let provider = text(identity, "provider");
    let model = model(identity);
    json!({"key": format!("{provider}/{model}"), "provider": provider, "model": model,
        "label": label(Some(provider)), "seat": text(identity, "seat")})
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
            .map(|r| text(r, "key"))
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
    live: bool,
    status: &'static str,
    latest_end: Option<(i64, String)>,
    color: u64,
    conversations: Vec<Value>,
    members: BTreeMap<String, Member>,
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
            live: false,
            status: "idle",
            conversations: Vec::new(),
            members: BTreeMap::new(),
            messages: Vec::new(),
            typing: BTreeMap::new(),
            sources: Vec::new(),
            include_messages: false,
        }
    }

    /// Conversations saved before hosts were recorded have neither host nor manager session.
    fn manager_label(&self) -> &str {
        match self.host.as_deref() {
            None if self.id.starts_with("c-") => "未记录主控",
            host => label(host),
        }
    }

    fn header(&self) -> Value {
        json!({"group": self.id, "host": self.host, "label": self.manager_label(),
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

    fn message(
        id: String,
        time: &str,
        conversation: &str,
        kind: &str,
        who: Value,
        body: &str,
    ) -> Value {
        json!({"id": id, "time": time, "conversation": conversation, "kind": kind,
            "speaker": who, "to": [], "text": body, "status": "", "detail": ""})
    }

    fn member(
        &mut self,
        snapshot: &Snapshot,
        identity: &Value,
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
            Some(message)
        } else {
            None
        };
        if let Some(typing) = typing
            && self
                .typing
                .get(&key)
                .is_none_or(|(old_at, _)| at >= *old_at)
        {
            self.typing.insert(key.clone(), (at, typing));
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
            self.title.clone_from(&item.title);
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
        for index in 0..self.sources.len() {
            let snapshot = &snapshots[self.sources[index]];
            if snapshot.item.mode == "task" {
                self.tasks(snapshot);
            } else {
                self.show(snapshot);
            }
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
                route_to(&mut msg, vec![recipient(task)]);
                self.messages.push(msg);
            }
            self.member(
                snapshot,
                task,
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
                    self.messages.push(output);
                }
                let body = if status == "succeeded" {
                    text(task, "output").to_owned()
                } else if status == "review" {
                    "待审核，请在主控中决定".into()
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
                self.messages.push(msg);
            }
        }
    }

    fn show(&mut self, snapshot: &Snapshot) {
        let item = &snapshot.item;
        let view = &snapshot.view["director"];
        let seats: BTreeMap<_, _> = array(view, "seats").map(|s| (text(s, "id"), s)).collect();
        let mut latest = BTreeMap::new();
        for (index, entry) in array(view, "messages").enumerate() {
            let kind = text(entry, "kind");
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
            if !manager && seat.is_some() {
                self.member(snapshot, &identity, status, text(entry, "text"), "", &time);
                latest.insert(text(entry, "speaker"), entry);
            }
            if !self.include_messages {
                continue;
            }
            let who = if manager {
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
                if manager {
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
            if manager {
                let recipients = if entry["recipients"].is_array() {
                    array(entry, "recipients").map(recipient).collect()
                } else {
                    seats.values().map(|s| recipient(s)).collect()
                };
                route_to(&mut msg, recipients);
            } else {
                msg["status"] = json!(status);
                if seat.is_some() {
                    msg["focus"] = json!(format!("seat:{}:{}", item.id, text(entry, "speaker")));
                }
            }
            self.messages.push(msg);
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
                &identity,
                "succeeded",
                text(entry, "text"),
                "",
                &time,
            );
            latest.insert(id, entry);
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
                msg["status"] = json!("succeeded");
                msg["focus"] = json!(format!("seat:{}:{id}", item.id));
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
                    &identity,
                    status,
                    text(run, "output"),
                    text(run, "error"),
                    &time,
                );
                if self.include_messages
                    && (status == "review"
                        || (committed.is_none()
                            && matches!(
                                status,
                                "failed" | "blocked" | "cancelled" | "interrupted" | "succeeded"
                            )
                            && run["review"] != "discard"))
                {
                    let body = if status == "review" {
                        "待审核，请在主控中决定".into()
                    } else if status == "succeeded" {
                        text(run, "output").to_owned()
                    } else {
                        failure(status).into()
                    };
                    let who = member_speaker(&identity);
                    let mut msg = Self::message(
                        format!("{}:round:{round}:{id}:result", item.id),
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
                    msg["detail"] = json!(text(run, "error"));
                    msg["focus"] = json!(format!("seat:{}:{id}", item.id));
                    self.messages.push(msg);
                }
            } else if !latest.contains_key(id) {
                self.member(
                    snapshot,
                    seat,
                    "pending",
                    "",
                    "",
                    &event_time("", &item.updated),
                );
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

pub(crate) fn respond(path: &str, mut snapshots: Vec<Snapshot>, data_exists: bool) -> Response {
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
        let mut value = group.header();
        value.as_object_mut().unwrap().remove("updated");
        value["conversations"] = json!(group.conversations);
        value["members"] = json!(group.members());
        value["messages"] = json!(group.timeline(focus));
        return Response::json(&value);
    }
    if path == "/app/stage" {
        let managers: Vec<_> = groups
            .into_iter()
            .filter(|g| stage_ids.contains(&g.id))
            .map(|g| {
                let mut value = g.header();
                value["members"] = json!(g.members());
                value
            })
            .collect();
        let members = || managers.iter().flat_map(|m| array(m, "members"));
        let running = members().filter(|m| m["status"] == "running").count();
        let review = members().filter(|m| m["status"] == "review").count();
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
        let groups: Vec<_> = groups
            .into_iter()
            .take(200)
            .map(|mut g| {
                g.project(&snapshots, true);
                let mut value = g.header();
                value["title"] = json!(g.title);
                value["conversations"] = json!(g.conversations.len());
                value["preview"] = json!(
                    g.timeline(None)
                        .last()
                        .map(|m| line("succeeded", text(m, "text"), ""))
                        .unwrap_or_default()
                );
                value
            })
            .collect();
        Response::json(&json!({"groups": groups}))
    }
}
