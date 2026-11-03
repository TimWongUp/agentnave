//! App-local reminders. Observe saved task/round transitions even while reminders are off,
//! so enabling them never replays work that ended earlier in this App process.

use std::collections::HashSet;

use serde_json::{Map, Value, json};
use tauri::AppHandle;
use tauri_plugin_notification::NotificationExt;

use crate::data::{Reader, Response};

mod permission;
mod projection;
#[cfg(test)]
mod tests;

pub(crate) use projection::observations;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Outcome {
    Completed,
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
    outcome: Outcome,
    elapsed_ms: Option<u64>,
}

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
            // `system` follows the contract's initial Chinese fallback.
            english: document
                .get("general")
                .and_then(|v| v.get("language"))
                .and_then(Value::as_str)
                == Some("en"),
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

#[derive(Debug, PartialEq)]
struct Notice {
    title: String,
    body: String,
}

impl Observation {
    fn notice(&self, english: bool) -> Notice {
        let status = match (self.outcome, english) {
            (Outcome::Completed, false) => "完成",
            (Outcome::Failed, false) => "失败",
            (Outcome::Blocked, false) => "受阻",
            (Outcome::Completed, true) => "Completed",
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
pub(crate) struct Monitor {
    initialized: bool,
    seen: HashSet<String>,
}

impl Monitor {
    fn observe(&mut self, events: Vec<Observation>, options: &Options) -> Vec<Notice> {
        let mut notices = Vec::new();
        for event in events {
            if self.seen.insert(event.key.clone()) && self.initialized && options.accepts(&event) {
                notices.push(event.notice(options.english));
            }
        }
        self.initialized = true;
        notices
    }

    pub(crate) fn tick(&mut self, app: &AppHandle, reader: &Reader) {
        let Ok(events) = reader.alert_observations() else {
            return;
        };
        let document = crate::preferences::read(&crate::preferences::dir()).unwrap_or_default();
        for notice in self.observe(events, &Options::from(&document)) {
            if let Err(reason) = send(app, &notice) {
                eprintln!("agentnave: notification: {reason}");
            }
        }
    }
}

fn send(app: &AppHandle, notice: &Notice) -> Result<(), String> {
    permission::check(app)?;
    let notification = app.notification();
    notification
        .builder()
        .title(&notice.title)
        .body(&notice.body)
        .show()
        .map_err(|e| e.to_string())
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
    let result = match send(app, &notice) {
        Ok(()) => json!({"sent": true}),
        Err(reason) => json!({"sent": false, "reason": reason}),
    };
    Response::json(&result)
}
