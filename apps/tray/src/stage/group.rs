use super::*;

impl Group {
    pub(super) fn new(id: String) -> Self {
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
            origin: None,
            project_name: None,
            project: None,
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
            isolated: Vec::new(),
        }
    }
    /// A workspace a boss opened by handing over: only that boss, no desks or conversations yet.
    pub(super) fn only_boss(id: String, handover: &Handover) -> Self {
        let mut group = Self::new(id);
        group.host.clone_from(&handover.host);
        group.title = handover.title.clone().unwrap_or_default();
        group.project_name = handover.cwd.as_deref().and_then(project_name);
        group.origin = handover
            .cwd
            .clone()
            .map(|cwd| (i64::MIN, String::new(), 0, cwd));
        if let Some(released) = &handover.released {
            group.updated.clone_from(released);
            group.at = timestamp(released).unwrap_or_default();
        }
        group
    }
    pub(super) fn manager_label(&self) -> &str {
        boss_name(self.host.as_deref(), self.app)
            .1
            .unwrap_or(label(self.host.as_deref()))
    }
    pub(super) fn header(&self) -> Value {
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
        // A workspace handed on again keeps its former boss while it waits for the next one.
        let previous_host = self
            .handover
            .previous_host
            .as_deref()
            .filter(|_| self.handover.changed);
        let project = self
            .project
            .as_ref()
            .filter(|p| p.key != crate::projects::OTHER);
        json!({"group": self.id, "host": host, "app": app, "label": label, "handover": handover,
            "took_over": took_over, "previous_host": previous_host,
            "previous_label": previous_host.map(|_| previous), "project": self.project_name, "title": self.title,
            "title_id": self.first_title.as_ref().map(|(_, _, id)| id)
                .or(self.first_id.as_ref().map(|(_, id)| id)),
            "color": self.color, "live": self.live, "status": self.status, "updated": self.updated,
            "isolated_count": self.isolated.len(),
            "project_key": project.map_or(crate::projects::OTHER, |p| p.key.as_str()),
            "project_label": project.map(|p| &p.label), "project_path": project.map(|p| &p.path),
            "boss_key": format!("{}|{}", host.unwrap_or_default(), label)})
    }
    pub(super) fn members(&self) -> Vec<Value> {
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
    pub(super) fn timeline(&self, focus: Option<&str>, complete: bool) -> Vec<Value> {
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
                complete
                    || *index >= skip
                    || focus.is_some_and(|id| text(message, "conversation") == id)
            })
            .map(|(_, message)| message.clone())
            .collect()
    }
    /// Office identities survive the message window without sending older prompt bodies.
    /// Keep each host's latest activity and last known model on either side of the handover.
    pub(super) fn boss_messages(&self) -> Vec<Value> {
        let boundary = (!self.handover.pending)
            .then(|| self.handover.released.as_deref().and_then(timestamp))
            .flatten();
        let mut prompts: Vec<_> = self
            .messages
            .iter()
            .filter(|m| m["speaker"]["role"] == "manager" && m["kind"] == "prompt")
            .collect();
        prompts.sort_by_key(|m| timestamp(text(m, "time")));
        let mut identities = BTreeMap::<(&str, bool), Value>::new();
        for message in prompts {
            let after = boundary
                .is_some_and(|at| timestamp(text(message, "time")).is_some_and(|time| time >= at));
            let identity = identities
                .entry((text(&message["speaker"], "provider"), after))
                .or_insert_with(|| json!({"kind": "prompt", "speaker": message["speaker"]}));
            identity["time"] = message["time"].clone();
            if !text(&message["speaker"], "model").is_empty() {
                identity["speaker"] = message["speaker"].clone();
            }
        }
        let mut messages: Vec<_> = identities.into_values().collect();
        messages.extend(
            self.messages
                .iter()
                .filter(|m| m["kind"] == "handover")
                .map(|m| json!({"kind": "handover", "time": m["time"]})),
        );
        messages.sort_by_key(|m| timestamp(text(m, "time")));
        messages
    }
    /// The desks an office draws, at most `limit` (the App's running limit) unless more are
    /// running or under review, plus the rest as the "optimized" list, most recent first. A
    /// desk whose failure has settled goes there whatever the limit. With `panel`, `limit` counts
    /// only the desks beyond the running ones, seats and settled failures count like any other
    /// idle desk, and both selection and order follow when the boss last messaged each desk. The
    /// list holds the `listed` most recent and every one the payload refers to, so seats and
    /// messages always find their desk.
    pub(super) fn desks(
        &self,
        host: Option<&str>,
        limit: usize,
        panel: bool,
        listed: usize,
        referenced: &BTreeSet<String>,
    ) -> (Vec<Value>, Value) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_micros() as i64);
        let (mut rest, mut desks): (Vec<_>, Vec<_>) = self
            .desks
            .values()
            .partition(|desk| !panel && desk.failure_settled(now));
        let recent = |d: &Desk| if panel { d.prompted() } else { d.last };
        desks.sort_by_key(|d| (!d.active, Reverse(recent(d)), &d.key));
        let busy = desks.iter().filter(|d| d.active).count();
        let keep = if panel { busy + limit } else { limit.max(busy) };
        rest.extend(desks.split_off(keep.min(desks.len())));
        rest.sort_by_key(|d| (Reverse(recent(d)), &d.key));
        if !panel {
            // Drawn desks put the newest first; the others keep their order as new ones arrive.
            desks.sort_by_key(|d| (Reverse(d.first.unwrap_or_default()), &d.key));
        }
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
    pub(super) fn rooms(&self) -> Vec<Value> {
        let mut rooms: Vec<_> = self.rooms.iter().collect();
        rooms.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
        rooms.into_iter().map(|(_, _, room)| room.clone()).collect()
    }
    /// Record one seat observation; `created` anchors first_seen so ordering stays stable.
    pub(super) fn desk_seat(
        &mut self,
        (room, seat): (&str, &str),
        identity: &Value,
        status: &str,
        (created, prompted, at): (i64, Option<i64>, i64),
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
        desk.prompt(prompted);
        desk.declare(created, opt_text(identity, "access"));
        desk.seat(at, status, meeting.then_some((room, seat)));
        desk.reported(at, identity);
    }
    pub(super) fn message(
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
    pub(super) fn member(
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
        let typing = self.typing_message(snapshot, (desk, identity), status, output, &key, who);
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
            value: json!({"key": key, "provider": provider, "model": model, "label": cli_label(identity, provider),
            "status": status, "line": line(status, output, error), "updated": time, "conversation": snapshot.item.id}),
        };
        if self.members.get(&key).is_none_or(|old| at >= old.at) {
            self.members.insert(key, member);
        }
    }
    pub(super) fn add(&mut self, snapshot: &Snapshot, index: usize) {
        let item = &snapshot.item;
        let at = timestamp(&item.updated).unwrap_or_default();
        if at >= self.at {
            self.at = at;
            self.updated.clone_from(&item.updated);
            self.host.clone_from(&item.host);
        }
        // Later calls may work elsewhere; the workspace stays with where it began.
        if let Some(origin) = origin::snapshot_origin(snapshot)
            && self.origin.as_ref().is_none_or(|known| origin < *known)
        {
            self.project_name = project_name(&origin.3);
            self.origin = Some(origin);
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
    pub(super) fn project(&mut self, snapshots: &[Snapshot], include_messages: bool) {
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
    pub(super) fn finish_messages(&mut self) {
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
            finish_message(msg, &prompts);
        }
        self.name_forward_sources();
    }
    /// A private task's forwards carry no source text; name each from the message it quotes, as
    /// a meeting's forward does. A task prompt and its reply share one id: the reply wins.
    fn name_forward_sources(&mut self) {
        let mut sources = HashMap::new();
        for m in &self.messages {
            let Some(id) = m["message_id"].as_str() else {
                continue;
            };
            if m["kind"] == "prompt" && sources.contains_key(id) {
                continue;
            }
            let who = &m["speaker"];
            let name = [text(who, "handle"), text(who, "label")]
                .into_iter()
                .find(|name| !name.is_empty())
                .unwrap_or_default();
            sources.insert(id.to_owned(), format!("{name} · {}", text(m, "time")));
        }
        for m in &mut self.messages {
            for forward in m
                .get_mut("forwards")
                .and_then(Value::as_array_mut)
                .into_iter()
                .flatten()
            {
                if let Some(source) = forward["message_id"]
                    .as_str()
                    .and_then(|id| sources.get(id))
                {
                    forward["source"] = json!(source);
                }
            }
        }
    }
    pub(super) fn omitted(&self, focus: Option<&str>, complete: bool) -> usize {
        if complete {
            return 0;
        }
        let total = self.messages.len() + self.typing.len();
        let Some(focus) = focus else {
            return total.saturating_sub(300);
        };
        if total <= 300 {
            return 0;
        }
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
        messages[..skip]
            .iter()
            .filter(|message| text(message, "conversation") != focus)
            .count()
    }
    pub(super) fn threads(&self) -> Vec<Value> {
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
                if thread["kind"] != "desk" {
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
    pub(super) fn tasks(&mut self, snapshot: &Snapshot) {
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
            let message = TaskMessage {
                id: &id,
                desk: &desk,
                time: &time,
                status,
                message_id: message_id.as_deref(),
            };
            self.observe_task_desk(item, task, &task_id, raw_start, &message);
            self.task_approvals(item, task, &id, &desk, &time);
            if self.include_messages
                && let Some(prompt) = task["prompt"].as_str()
            {
                let mut msg = Self::message(
                    format!("{id}:prompt"),
                    &start,
                    &item.id,
                    "prompt",
                    manager_speaker(task, item.host.as_deref(), self.manager_label()),
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
                let forwards: Vec<_> = array(task, "forward")
                    .filter_map(Value::as_str)
                    .map(|id| json!({"message_id": id, "source": ""}))
                    .collect();
                if !forwards.is_empty() {
                    msg["forwards"] = json!(forwards);
                }
                if let Some(branch) = opt_text(task, "isolation_branch") {
                    msg["isolation_branch"] =
                        json!(branch.strip_prefix("refs/heads/").unwrap_or(&branch));
                }
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
            self.task_result(item, task, &message);
        }
    }
    fn typing_message(
        &self,
        snapshot: &Snapshot,
        (desk, identity): (&str, &Value),
        status: &str,
        output: &str,
        key: &str,
        who: Value,
    ) -> Option<(String, Value)> {
        if self.include_messages && status == "running" {
            let tail: Vec<_> = output
                .lines()
                .rev()
                .filter(|s| !s.trim().is_empty())
                .take(4)
                .collect();
            let tail = tail.into_iter().rev().collect::<Vec<_>>().join("\n");
            let mut message = Self::message(
                key.to_owned(),
                &event_time(
                    snapshot.live_updated.as_deref().unwrap_or(""),
                    &snapshot.item.updated,
                ),
                &snapshot.item.id,
                "typing",
                who,
                &tail,
            );
            crate::data::copy_progress(&mut message, identity);
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
        }
    }
    fn observe_task_desk(
        &mut self,
        item: &Item,
        task: &Value,
        task_id: &str,
        raw_start: &str,
        message: &TaskMessage<'_>,
    ) {
        let TaskMessage {
            desk, time, status, ..
        } = *message;
        let provider = text(task, "provider");
        if !provider.is_empty() {
            let entry = self.desks.entry(desk.to_owned()).or_insert_with(|| Desk {
                key: desk.to_owned(),
                provider: provider.to_owned(),
                ..Desk::default()
            });
            let started = timestamp(raw_start);
            let started_at = started.unwrap_or_default();
            entry.label(
                started_at,
                text(task, "model"),
                opt_text(task, "effort"),
                desk_role(task),
            );
            entry.identify(started_at, &task["identity"]);
            entry.seen(started_at);
            entry.prompt(started);
            entry.declare(started_at, opt_text(task, "access"));
            entry.observe(timestamp(time).unwrap_or_default(), status);
            entry.reported(timestamp(time).unwrap_or_default(), task);
            if active(status) {
                entry
                    .activities
                    .push(json!({"kind": "private", "status": status, "conversation": item.id}));
            }
            entry.tasks += 1;
            if desk.starts_with("thread:") {
                entry.aliases.push(format!("task:{}:{task_id}", item.id));
            }
            if status == "running" {
                entry.running += 1;
            }
        }
    }
    fn task_result(&mut self, item: &Item, task: &Value, message: &TaskMessage<'_>) {
        let TaskMessage {
            id,
            desk,
            time,
            status,
            message_id,
        } = *message;
        let desk_ref = (!text(task, "provider").is_empty()).then_some(desk);
        if self.include_messages && status != "running" && status != "pending" {
            let who = member_speaker(task);
            // Failed calls can still contain a useful native answer; keep it beside the
            // failure notice now that chat is the App's only history reader.
            if !matches!(status, "succeeded" | "review") && !text(task, "output").is_empty() {
                let mut output = Self::message(
                    format!("{id}:output"),
                    time,
                    &item.id,
                    "reply",
                    who.clone(),
                    text(task, "output"),
                );
                output["status"] = json!(status);
                annotate(
                    &mut output,
                    "private",
                    desk_thread(desk),
                    desk_ref,
                    message_id,
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
                time,
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
                desk_thread(desk),
                desk_ref,
                message_id,
                None,
            );
            self.messages.push(msg);
        }
    }
}

fn finish_message(msg: &mut Value, prompts: &BTreeMap<(String, String), Value>) {
    let conversation = text(msg, "conversation").to_owned();
    // A round that called some seats, an instruction and a private task keep their addressees.
    let addressed = msg["answering"].is_array()
        || msg["kind"] == "instruction"
        || (msg["kind"] == "prompt" && msg["thread"]["kind"] == "desk");
    if !addressed {
        msg["to"] = if msg["visibility"] == "public" && msg["thread"]["kind"] == "room" {
            json!([{"key": "@all", "handle": "@all"}])
        } else {
            json!([{"key": "@boss", "handle": "@boss"}])
        };
    }
    if msg["speaker"]["role"] != "manager" || msg["kind"] == "instruction" {
        if msg["thread"]["kind"] == "room" {
            msg["reply_to"] = prompts
                .get(&(conversation, msg["round"].to_string()))
                .cloned()
                .unwrap_or(Value::Null);
        } else if !matches!(text(msg, "kind"), "typing" | "approval")
            && let Some((prefix, _)) = text(msg, "id").rsplit_once(':')
        {
            msg["reply_to"] = json!(format!("{prefix}:prompt"));
        }
    }
}

struct TaskMessage<'a> {
    id: &'a str,
    desk: &'a str,
    time: &'a str,
    status: &'a str,
    message_id: Option<&'a str>,
}
