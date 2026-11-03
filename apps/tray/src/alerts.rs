//! App-local reminders. Observe saved task/round transitions even while reminders are off,
//! so enabling them never replays work that ended earlier in this App process.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::thread;

use serde_json::{Map, Value, json};
use tauri::AppHandle;

use crate::data::{Reader, Response};

mod permission;
mod projection;
#[cfg(test)]
mod tests;

pub(crate) use projection::observations;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Outcome {
    Completed,
    RoundEnded,
    Review,
    Failed,
    Blocked,
}

/// A notification-worthy event, before settings and the startup baseline are applied.
#[derive(Clone)]
pub(crate) struct Observation {
    key: String,
    title: String,
    meeting: bool,
    problem: bool,
    unstamped_problem: bool,
    outcome: Outcome,
    elapsed_ms: Option<u64>,
}

/// Readable conversations, including those without any finished work yet.
pub(crate) type Readings = HashMap<String, Vec<Observation>>;

struct Options {
    enabled: bool,
    meetings_only: bool,
    threshold_ms: u64,
    problems: bool,
    english: bool,
}

impl Options {
    fn from(document: &Map<String, Value>) -> Self {
        let alerts = document.get("alerts");
        let field = |name: &str| alerts.and_then(|value| value.get(name));
        Self {
            enabled: field("notify").and_then(Value::as_str) == Some("system"),
            meetings_only: field("scope").and_then(Value::as_str) == Some("meetings"),
            threshold_ms: field("threshold")
                .and_then(Value::as_u64)
                .filter(|n| [0, 10, 30].contains(n))
                .unwrap_or(10)
                * 60_000,
            problems: field("problems").and_then(Value::as_bool).unwrap_or(false),
            english: crate::preferences::general::resolved_language(
                document
                    .get("general")
                    .and_then(|v| v.get("language"))
                    .and_then(Value::as_str)
                    .unwrap_or("system"),
                &sys_locale::get_locale().unwrap_or_default(),
            ) == "en",
        }
    }

    fn accepts(&self, event: &Observation) -> bool {
        self.enabled
            && (!self.meetings_only || event.meeting)
            && if event.problem {
                self.problems
            } else {
                event.elapsed_ms.is_some_and(|ms| ms >= self.threshold_ms)
            }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Notice {
    title: String,
    body: String,
}

impl Observation {
    fn notice(&self, english: bool) -> Notice {
        let status = match (self.outcome, english) {
            (Outcome::Completed, false) => "完成",
            (Outcome::RoundEnded, false) => "本轮结束",
            (Outcome::Review, false) => "待审核",
            (Outcome::Failed, false) => "失败",
            (Outcome::Blocked, false) => "受阻",
            (Outcome::Completed, true) => "Completed",
            (Outcome::RoundEnded, true) => "Round ended",
            (Outcome::Review, true) => "Awaiting review",
            (Outcome::Failed, true) => "Failed",
            (Outcome::Blocked, true) => "Blocked",
        };
        let duration = match self.elapsed_ms {
            Some(ms) if english => format!("{}m {}s", ms / 60_000, ms / 1_000 % 60),
            Some(ms) => format!("{}分{}秒", ms / 60_000, ms / 1_000 % 60),
            None if english => "duration unavailable".into(),
            None => "耗时未知".into(),
        };
        Notice {
            title: self.title.clone(),
            body: format!("{status} · {duration}"),
        }
    }
}

#[derive(Default)]
struct Conversation {
    seen: HashSet<String>,
    unstamped_problems: HashSet<String>,
}

impl Conversation {
    fn observe(
        &mut self,
        events: Vec<Observation>,
        options: &Options,
        baseline: bool,
    ) -> Vec<(String, Notice)> {
        // Launch errors may have no saved finish time. An observed return to running
        // rearms only these events; timestamped results already identify each attempt.
        // A retry wholly between polls cannot be distinguished without a saved attempt id
        // when neither failure has a finish time.
        let current: HashSet<_> = events
            .iter()
            .filter(|e| e.unstamped_problem)
            .map(|e| e.key.clone())
            .collect();
        for key in self.unstamped_problems.difference(&current) {
            self.seen.remove(key);
        }
        self.unstamped_problems = current;
        let mut notices = Vec::new();
        for event in events {
            if self.seen.contains(&event.key) {
                continue;
            }
            if baseline {
                self.seen.insert(event.key);
            } else if options.accepts(&event) {
                notices.push((event.key.clone(), event.notice(options.english)));
            } else if !options.enabled
                || (options.meetings_only && !event.meeting)
                || (event.problem && !options.problems)
                || event.elapsed_ms.is_some()
            {
                // Final events filtered by user settings are never replayed. Missing
                // duration can still be repaired, so leave that event eligible.
                self.seen.insert(event.key);
            }
        }
        notices
    }
}

#[derive(Default)]
pub(crate) struct Monitor {
    conversations: HashMap<String, Conversation>,
}

impl Monitor {
    pub(crate) fn tick(&mut self, app: &AppHandle, reader: &Reader) {
        let Ok(events) = reader.alert_observations() else {
            return;
        };
        let document = crate::preferences::read(&crate::preferences::dir()).unwrap_or_default();
        let options = Options::from(&document);
        self.deliver(events, &options, |notice| {
            send(app, notice, false, options.english)
        });
    }

    fn deliver(
        &mut self,
        readings: Readings,
        options: &Options,
        mut send: impl FnMut(&Notice) -> Result<(), String>,
    ) {
        for (id, events) in readings {
            let baseline = !self.conversations.contains_key(&id);
            let conversation = self.conversations.entry(id).or_default();
            for (key, notice) in conversation.observe(events, options, baseline) {
                let result = send(&notice);
                conversation.seen.insert(key);
                if let Err(reason) = result {
                    eprintln!("agentnave: notification: {reason}");
                }
            }
        }
    }
}

/// Native permission callbacks must not stall tray counts or keep-awake polling.
pub(crate) fn start(app: AppHandle, reader: Arc<Reader>) {
    thread::spawn(move || {
        let mut monitor = Monitor::default();
        loop {
            monitor.tick(&app, &reader);
            thread::sleep(crate::POLL);
        }
    });
}

fn send(app: &AppHandle, notice: &Notice, request: bool, english: bool) -> Result<(), String> {
    permission::check(app, request, english)?;
    permission::send(app, &notice.title, &notice.body, english)
}

/// Test delivery uses the same notification switch and native permission checks.
pub(crate) fn test(app: &AppHandle, method: &str, action: Option<&str>) -> Response {
    if method != "POST" {
        return Response::error(405, "此操作只接受 POST");
    }
    if action != Some("1") {
        return Response::error(403, "缺少操作请求头");
    }
    let document = crate::preferences::read(&crate::preferences::dir()).unwrap_or_default();
    let options = Options::from(&document);
    let english = options.english;
    if !options.enabled {
        return Response::json(&json!({"sent": false, "reason": if english {
            "Notifications are turned off"
        } else { "提醒已关闭" }}));
    }
    let notice = Notice {
        title: "AgentNave".into(),
        body: if english {
            "Test notification"
        } else {
            "测试通知"
        }
        .into(),
    };
    let result = match send(app, &notice, true, english) {
        Ok(()) => json!({"sent": true}),
        Err(reason) => json!({"sent": false, "reason": reason}),
    };
    Response::json(&result)
}
