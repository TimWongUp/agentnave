//! A desk in the office: one native session and what its calls have shown.
use super::*;

/// A desk seen in one workspace: one native session (a private thread or a show seat), with
/// the labels of its latest call and its history.
#[derive(Default)]
pub(super) struct Desk {
    pub(super) key: String,
    // The room a seat desk belongs to; None for a private thread.
    pub(super) room: Option<String>,
    pub(super) provider: String,
    pub(super) model: String,
    pub(super) effort: Option<String>,
    pub(super) role: Option<String>,
    pub(super) labelled: Option<i64>,
    // Several desks with the same model, CLI and role are numbered by first appearance.
    pub(super) ordinal: usize,
    pub(super) first: Option<i64>,
    pub(super) last: Option<i64>,
    pub(super) latest: Option<(i64, String)>,
    pub(super) active: bool,
    pub(super) tasks: usize,
    pub(super) running: usize,
    pub(super) activities: Vec<Value>,
    // A thread desk's earlier `task:` keys, from before its tasks had a native session.
    pub(super) aliases: Vec<String>,
    pub(super) reported: Option<(i64, String)>,
    // The latest declared access tier; the server records declarations without enforcing them.
    pub(super) access: Option<(i64, String)>,
}

impl Desk {
    /// Model and role follow the desk's latest call; a resumed thread may relabel it.
    pub(super) fn label(
        &mut self,
        at: i64,
        model: &str,
        effort: Option<String>,
        role: Option<String>,
    ) {
        if self.labelled.is_none_or(|old| at >= old) {
            self.labelled = Some(at);
            self.model = model.to_owned();
            self.effort = effort;
            self.role = role;
        }
    }

    pub(super) fn base(&self) -> (String, String, Option<String>) {
        (
            self.display_model(),
            self.provider.clone(),
            self.role.clone(),
        )
    }

    pub(super) fn declare(&mut self, at: i64, access: Option<String>) {
        if let Some(access) = access
            && self.access.as_ref().is_none_or(|(old_at, _)| at >= *old_at)
        {
            self.access = Some((at, access));
        }
    }

    pub(super) fn seen(&mut self, at: i64) {
        self.first = Some(self.first.map_or(at, |first| first.min(at)));
        self.last = Some(self.last.map_or(at, |last| last.max(at)));
    }

    pub(super) fn observe(&mut self, at: i64, status: &str) {
        self.seen(at);
        let status = member_status(status);
        if active(status) {
            self.active = true;
        } else if self.latest.as_ref().is_none_or(|(old, _)| at >= *old) {
            self.latest = Some((at, status.to_owned()));
        }
    }

    pub(super) fn seat(&mut self, at: i64, status: &str, meeting: Option<(&str, &str)>) {
        self.observe(at, status);
        let status = member_status(status);
        if let Some((room, seat)) = meeting
            && active(status)
        {
            self.activities
                .push(json!({"kind": "meeting", "room": room, "seat": seat, "status": status}));
        }
    }

    pub(super) fn reported(&mut self, at: i64, identity: &Value) {
        if let Some(model) = opt_text(identity, "reported_model")
            && self.reported.as_ref().is_none_or(|(old, _)| at >= *old)
        {
            self.reported = Some((at, model));
        }
    }

    /// The model the CLI reported for the latest call, unless a newer call has yet to report.
    fn reported_model(&self) -> Option<&str> {
        let (at, model) = self.reported.as_ref()?;
        (self.model.is_empty() || self.labelled.is_none_or(|labelled| *at >= labelled))
            .then_some(model.as_str())
    }

    /// The model the CLI reported, else the requested one, else the CLI's own default.
    pub(super) fn display_model(&self) -> String {
        if let Some(model) = self.reported_model() {
            model.to_owned()
        } else if !self.model.is_empty() {
            self.model.clone()
        } else {
            crate::detect::cached_native(&self.provider).unwrap_or_default()
        }
    }

    pub(super) fn model_source(&self) -> &'static str {
        if self.reported_model().is_some() {
            "native"
        } else if !self.model.is_empty() {
            "requested"
        } else if crate::detect::cached_native(&self.provider).is_some() {
            "cli_default"
        } else {
            "unknown"
        }
    }

    pub(super) fn numbered(&self, name: String) -> String {
        if self.ordinal == 0 {
            name
        } else {
            format!("{name} #{}", self.ordinal)
        }
    }

    pub(super) fn handle(&self) -> String {
        self.numbered(handle(
            &self.display_model(),
            self.effort.as_deref(),
            &self.provider,
            self.role.as_deref(),
        ))
    }

    pub(super) fn status(&self) -> &'static str {
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

    pub(super) fn value(&self, host: Option<&str>) -> Value {
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
            "effort": self.effort,
            "role": self.role,
            "access": self.access.as_ref().map(|(_, access)| access),
            "label": self.numbered(desk_label(&model_with_effort(&self.display_model(), self.effort.as_deref()), &self.provider, self.role.as_deref())),
            "display_model": self.display_model(),
            "model_source": self.model_source(),
            "handle": self.handle(), "activities": self.activities.iter().rev().collect::<Vec<_>>(),
            "first_seen": iso(self.first.unwrap_or_default()),
            "last_active": iso(self.last.unwrap_or_default()),
            "status": status,
            "private_allowed": host_provider(host) != Some(self.provider.as_str()),
            "private": {"tasks": self.tasks, "running": self.running},
            "aliases": self.aliases,
            "meeting": meeting,
        })
    }
}
