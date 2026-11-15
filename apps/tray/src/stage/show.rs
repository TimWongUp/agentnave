//! One meeting's director view projected onto members, desks, messages and its room.
use super::*;

/// What the passes over a director view share.
struct Show<'a> {
    snapshot: &'a Snapshot,
    item: &'a Item,
    view: &'a Value,
    seats: BTreeMap<&'a str, &'a Value>,
    round_times: BTreeMap<u64, i64>,
    created: i64,
}

impl<'a> Show<'a> {
    fn round_start(&self, round: u64) -> Option<i64> {
        self.round_times.get(&round).copied()
    }

    fn latest_round_start(&self, id: &str) -> Option<i64> {
        array(self.view, "rounds")
            .filter(|r| array(r, "seats").any(|s| text(s, "seat_id") == id))
            .filter_map(|r| self.round_start(r["number"].as_u64()?))
            .max()
    }
    /// A private answer carries its finish time; older records only have the round's start. The
    /// answer was done before a later round began, so the later of the two counts as the desk's
    /// work, and the conversation's last update when no round followed.
    fn next_round_start(&self, round: u64) -> Option<i64> {
        array(self.view, "messages")
            .filter(|m| m["round"].as_u64().is_some_and(|r| r > round))
            .filter_map(|m| timestamp(text(m, "time")))
            .min()
            .or_else(|| timestamp(&self.item.updated))
    }

    fn time(&self, entry: &Value) -> String {
        event_time(text(entry, "time"), &self.item.updated)
    }

    fn focus(&self, seat: &str) -> Value {
        json!(format!("seat:{}:{seat}", self.item.id))
    }

    /// The recipients of a boss message: those it names, or every seat.
    fn recipients(&self, entry: &Value) -> Vec<Value> {
        let conversation = &self.item.id;
        if entry["recipients"].is_array() {
            array(entry, "recipients")
                .filter_map(|r| {
                    let id = text(r, "seat");
                    let seat = self.seats.get(id)?;
                    Some(recipient(
                        &seat_desk(conversation, id),
                        &identity_of(seat, id, r),
                    ))
                })
                .collect()
        } else {
            self.seats
                .iter()
                .map(|(id, seat)| {
                    recipient(
                        &seat_desk(conversation, id),
                        &identity_of(seat, id, &Value::Null),
                    )
                })
                .collect()
        }
    }

    /// The seats a task round called, when that was not every seat; those not started yet
    /// included, the rest still hear it.
    fn answering(&self, entry: &Value) -> Option<Vec<Value>> {
        let round = (self.view["round"]["number"] == entry["round"])
            .then_some(&self.view["round"])
            .or_else(|| array(self.view, "rounds").find(|r| r["number"] == entry["round"]))?;
        let conversation = &self.item.id;
        let answering: Vec<_> = array(round, "seats")
            .filter_map(|r| {
                let id = text(r, "seat_id");
                let seat = self.seats.get(id)?;
                Some(recipient(
                    &seat_desk(conversation, id),
                    &identity_of(seat, id, r),
                ))
            })
            .collect();
        (answering.len() < self.seats.len()).then_some(answering)
    }

    /// What a seat goes by now: a running seat's tail may already name a newer model.
    fn current(&self, id: &str, seat: &'a Value) -> &'a Value {
        array(&self.view["round"], "seats")
            .find(|run| text(run, "seat_id") == id)
            .map(|run| &run["identity"])
            .filter(|identity| identity.is_object())
            .unwrap_or(&seat["identity"])
    }
}

/// A seat's record under its id, with the model and name `source` reported when it did.
pub(super) fn identity_of(seat: &Value, id: &str, source: &Value) -> Value {
    let mut identity = seat.clone();
    identity["seat"] = json!(id);
    if !text(source, "reported_model").is_empty() {
        identity["reported_model"] = source["reported_model"].clone();
    }
    if source["identity"].is_object() {
        identity["identity"] = source["identity"].clone();
    }
    crate::data::copy_progress(&mut identity, source);
    identity
}

/// A stored entry's seat: it signs with the name it recorded, never the seat's current one.
fn entry_identity(seat: &Value, id: &str, entry: &Value) -> Value {
    let mut identity = identity_of(seat, id, entry);
    identity["identity"] = entry["identity"].clone();
    identity
}

/// The status an absence reports, recognised from its text.
fn absence_status(entry: &Value) -> &'static str {
    let body = text(entry, "text");
    ["blocked", "cancelled", "interrupted"]
        .into_iter()
        .find(|s| body.contains(failure(s)) || body == *s)
        .unwrap_or("failed")
}

fn annotate_private(message: &mut Value, show: &Show<'_>, identity: &Value) {
    annotate(
        message,
        "private",
        room_thread(&show.item.id),
        desk_of(&show.item.id, identity).as_deref(),
        None,
        None,
    );
}

impl Group {
    pub(super) fn show(&mut self, snapshot: &Snapshot) {
        let item = &snapshot.item;
        let view = &snapshot.view["director"];
        let mut round_times = BTreeMap::new();
        for entry in array(view, "messages").filter(|m| text(m, "kind") == "task") {
            if let (Some(round), Some(at)) =
                (entry["round"].as_u64(), timestamp(text(entry, "time")))
            {
                round_times.entry(round).or_insert(at);
            }
        }
        let show = Show {
            snapshot,
            item,
            view,
            seats: array(view, "seats").map(|s| (text(s, "id"), s)).collect(),
            round_times,
            created: timestamp(text(view, "created"))
                .or_else(|| timestamp(&item.updated))
                .unwrap_or_default(),
        };
        let mut answered = BTreeSet::new();
        for (index, entry) in array(view, "messages").enumerate() {
            self.show_entry(&show, index, entry, &mut answered);
        }
        for entry in array(view, "instructions") {
            self.show_instruction(&show, entry);
        }
        for entry in array(view, "private_messages") {
            self.show_private(&show, entry, &mut answered);
        }
        for (id, seat) in &show.seats {
            let run = array(&view["round"], "seats").find(|r| text(r, "seat_id") == *id);
            if let Some(run) = run {
                self.show_run(&show, id, seat, run);
            } else if !answered.contains(id) {
                self.member(
                    snapshot,
                    (&seat_desk(&item.id, id), seat),
                    "pending",
                    "",
                    "",
                    &event_time("", &item.updated),
                );
                self.desk_seat(
                    (&item.id, id),
                    seat,
                    "pending",
                    (show.created, show.latest_round_start(id), show.created),
                    false,
                );
            }
            if let Some(desk) = self.desks.get_mut(&seat_desk(&item.id, id)) {
                desk.prompt(show.latest_round_start(id));
                desk.identity = show.current(id, seat).clone();
            }
        }
        self.show_room(&show);
        self.show_approvals(snapshot);
    }

    fn show_entry<'a>(
        &mut self,
        show: &Show<'_>,
        index: usize,
        entry: &'a Value,
        answered: &mut BTreeSet<&'a str>,
    ) {
        let kind = text(entry, "kind");
        let speaker = text(entry, "speaker");
        let seat = show.seats.get(speaker).copied();
        let identity = seat.map_or(Value::Null, |seat| entry_identity(seat, speaker, entry));
        let time = show.time(entry);
        let status = if kind == "absent" {
            absence_status(entry)
        } else {
            "succeeded"
        };
        if !matches!(kind, "task" | "host" | "forward") && seat.is_some() {
            self.member(
                show.snapshot,
                (&seat_desk(&show.item.id, speaker), &identity),
                status,
                text(entry, "text"),
                "",
                &time,
            );
            answered.insert(speaker);
            let prompted = entry["round"].as_u64().and_then(|r| show.round_start(r));
            self.desk_seat(
                (&show.item.id, speaker),
                &identity,
                status,
                (
                    show.created,
                    prompted,
                    timestamp(&time).unwrap_or(show.created),
                ),
                false,
            );
        }
        if self.include_messages {
            let message = self.entry_message(show, (index, entry), &identity, &time, status);
            self.messages.push(message);
        }
    }

    fn entry_message(
        &self,
        show: &Show<'_>,
        (index, entry): (usize, &Value),
        identity: &Value,
        time: &str,
        status: &str,
    ) -> Value {
        let conversation = &show.item.id;
        let kind = text(entry, "kind");
        let speaker_id = text(entry, "speaker");
        let has_seat = show.seats.contains_key(speaker_id);
        let forwarded = kind == "forward";
        let manager = matches!(kind, "task" | "host");
        let absent = kind == "absent";
        let host = show.item.host.as_deref();
        let who = if forwarded {
            let mut who = manager_speaker(entry, host, "");
            who["label"] = json!("boss 转发");
            who
        } else if manager {
            manager_speaker(entry, host, self.manager_label())
        } else if has_seat {
            member_speaker(identity)
        } else {
            speaker("system", "", "", "系统")
        };
        let mut msg = Self::message(
            format!("{conversation}:entry:{index}"),
            time,
            conversation,
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
        if kind != "host" && entry["round"].as_u64().is_some_and(|r| r > 0) {
            msg["round"] = entry["round"].clone();
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
            room_thread(conversation),
            entry_desk(conversation, entry, &show.seats).as_deref(),
            opt_text(entry, "message_id").as_deref(),
            forward,
        );
        if manager {
            route_to(&mut msg, show.recipients(entry));
            if kind == "task"
                && let Some(called) = show.answering(entry)
            {
                msg["answering"] = json!(called.iter().map(|r| &r["key"]).collect::<Vec<_>>());
                msg["to"] = json!(called.iter().map(addressee).collect::<Vec<_>>());
            }
        } else {
            msg["status"] = json!(status);
            if has_seat {
                msg["focus"] = show.focus(speaker_id);
            }
        }
        if kind == "answer" {
            msg["id"] = json!(format!(
                "{conversation}:round:{}:{speaker_id}:result",
                entry["round"],
            ));
        }
        msg
    }

    fn show_instruction(&mut self, show: &Show<'_>, entry: &Value) {
        let id = text(entry, "seat_id");
        let Some(seat) = show.seats.get(id) else {
            return;
        };
        if !self.include_messages {
            return;
        }
        let conversation = &show.item.id;
        let identity = entry_identity(seat, id, entry);
        let mut msg = Self::message(
            format!("{conversation}:instruction:{}:{id}", entry["round"]),
            &show.time(entry),
            conversation,
            "instruction",
            manager_speaker(entry, show.item.host.as_deref(), self.manager_label()),
            text(entry, "text"),
        );
        msg["round"] = entry["round"].clone();
        route_to(
            &mut msg,
            vec![recipient(&seat_desk(conversation, id), &identity)],
        );
        annotate_private(&mut msg, show, &identity);
        self.messages.push(msg);
    }

    fn show_private<'a>(
        &mut self,
        show: &Show<'_>,
        entry: &'a Value,
        answered: &mut BTreeSet<&'a str>,
    ) {
        let id = text(entry, "seat_id");
        let Some(seat) = show.seats.get(id).copied() else {
            return;
        };
        let conversation = &show.item.id;
        let time = show.time(entry);
        let identity = entry_identity(seat, id, entry);
        self.member(
            show.snapshot,
            (&seat_desk(conversation, id), &identity),
            "succeeded",
            text(entry, "text"),
            "",
            &time,
        );
        answered.insert(id);
        let round = entry["round"].as_u64().unwrap_or_default();
        let last = [show.next_round_start(round), timestamp(&time)]
            .into_iter()
            .flatten()
            .max()
            .unwrap_or(show.created);
        self.desk_seat(
            (conversation, id),
            &identity,
            "succeeded",
            (show.created, show.round_start(round), last),
            false,
        );
        if self.include_messages {
            let mut msg = Self::message(
                format!("{conversation}:round:{round}:{id}:private"),
                &time,
                conversation,
                "reply",
                member_speaker(&identity),
                text(entry, "text"),
            );
            msg["private"] = json!(true);
            msg["kept_private"] = json!(true);
            msg["round"] = entry["round"].clone();
            msg["status"] = json!("succeeded");
            msg["focus"] = show.focus(id);
            annotate_private(&mut msg, show, &identity);
            self.messages.push(msg);
        }
    }

    /// A seat in the current round: its live or finished state, and a message for what no
    /// committed entry shows yet.
    fn show_run(&mut self, show: &Show<'_>, id: &str, seat: &Value, run: &Value) {
        let item = show.item;
        let identity = identity_of(seat, id, run);
        let status = if run["review"] == "pending" {
            "review"
        } else {
            member_status(text(run, "status"))
        };
        let round = show.view["round"]["number"].as_u64().unwrap_or_default();
        let committed = array(show.view, "messages").rev().find(|e| {
            text(e, "speaker") == id
                && e["round"].as_u64() == Some(round)
                && matches!(text(e, "kind"), "answer" | "absent")
        });
        // Host messages change conversation activity, never the time of a committed answer.
        let time = if status == "running" {
            event_time(
                show.snapshot.live_updated.as_deref().unwrap_or(""),
                &item.updated,
            )
        } else {
            // An answer keeps the time it finished, before and after the round publishes it.
            let finished = committed.map_or(text(run, "finished"), |entry| text(entry, "time"));
            event_time(finished, &item.updated)
        };
        self.member(
            show.snapshot,
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
            (
                show.created,
                show.round_start(round),
                timestamp(&time).unwrap_or(show.created),
            ),
            true,
        );
        if self.include_messages
            && (status == "review"
                || (committed.is_none()
                    && matches!(
                        status,
                        "failed" | "blocked" | "cancelled" | "interrupted" | "succeeded"
                    )
                    && run["review"].as_str() != Some("private")))
        {
            let answered = matches!(status, "review" | "succeeded");
            let mut msg = Self::message(
                format!("{}:round:{round}:{id}:result", item.id),
                &time,
                &item.id,
                if answered { "reply" } else { "notice" },
                member_speaker(&identity),
                if answered {
                    text(run, "output")
                } else {
                    failure(status)
                },
            );
            msg["status"] = json!(status);
            msg["round"] = json!(round);
            msg["detail"] = json!(text(run, "error"));
            msg["focus"] = show.focus(id);
            annotate_private(&mut msg, show, &identity);
            self.messages.push(msg);
        }
    }

    fn show_room(&mut self, show: &Show<'_>) {
        let item = show.item;
        let view = show.view;
        let room_seats: Vec<Value> = show
            .seats
            .values()
            .filter(|seat| !text(seat, "provider").is_empty())
            .map(|seat| {
                let id = text(seat, "id");
                json!({"seat": id, "desk": seat_desk(&item.id, id), "identity": show.current(id, seat)})
            })
            .collect();
        self.rooms.push((
            show.created,
            item.id.clone(),
            json!({
                "id": item.id,
                "title": item.title,
                "status": conversation_status(show.snapshot),
                "last_active": item.updated,
                "seats": room_seats,
                "pending_review": array(&view["round"], "seats").filter(|run| run["review"] == "pending").count(),
                "round": view["round"].as_object().map(|r| json!({"number": r.get("number"), "order": r.get("order"), "status": r.get("status")})),
                "board": array(view, "messages").filter(|entry| text(entry, "kind") != "absent").count(),
            }),
        ));
    }
}
