//! Request envelopes, push receipt projections, and run handles for derived sessions (ADR-0109, ADR-0114).

use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Deserialize)]
pub(in crate::terminal) struct ProcessIdentity {
    pub pid: u32,
    pub started_at: f64,
}

#[derive(Clone, Debug, Deserialize)]
pub(in crate::terminal) struct ParentRef {
    pub boss_key: String,
    #[serde(default)]
    pub workspace_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub(in crate::terminal) struct IsolationSpec {
    #[serde(alias = "isolation_id")]
    pub id: String,
    #[serde(default)]
    pub repo_common_dir: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub(in crate::terminal) struct RunConfig {
    pub provider: String,
    #[serde(default)]
    pub account: Option<String>,
    #[serde(default)]
    pub account_dir: Option<String>,
    pub cwd: String,
    #[serde(default)]
    pub isolation: Option<IsolationSpec>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
    #[serde(default)]
    pub permissions: Value,
}

#[derive(Clone, Debug, Deserialize)]
pub(in crate::terminal) struct ForwardItem {
    #[serde(default)]
    pub message_id: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub text: String,
}

#[derive(Clone, Debug, Deserialize)]
pub(in crate::terminal) struct TaskSpec {
    pub text: String,
    #[serde(default)]
    pub forward: Vec<ForwardItem>,
}

#[derive(Clone, Debug, Deserialize)]
pub(in crate::terminal) struct ExpectedAnchor {
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub turn_id: Option<String>,
    #[serde(default)]
    pub client_user_message_id: Option<String>,
    #[serde(default)]
    pub prompt_id: Option<String>,
    #[serde(default)]
    pub user_input_index: Option<i64>,
}

#[derive(Clone, Debug, Deserialize)]
pub(in crate::terminal) struct SpawnRequestEnvelope {
    pub version: u32,
    pub request_id: String,
    pub kind: String,
    pub service: ProcessIdentity,
    pub target_tray: ProcessIdentity,
    pub invocation_id: String,
    #[serde(default)]
    pub parent: Option<ParentRef>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub config: Option<RunConfig>,
    #[serde(default)]
    pub task: Option<TaskSpec>,
    #[serde(default)]
    pub expected_anchor: Option<ExpectedAnchor>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::terminal) enum ReceiptState {
    Accepted,
    Started,
    Finished,
}

#[derive(Clone, Debug, Serialize)]
pub(in crate::terminal) struct BlockedReceipt {
    pub kind: String,
    pub since: f64,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize)]
pub(in crate::terminal) struct BackgroundTasksReminder {
    pub code: String,
    pub since: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<usize>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tasks: Vec<String>,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize)]
pub(in crate::terminal) struct TerminalOutcome {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(in crate::terminal) struct Receipt {
    pub service_pid: u32,
    pub invocation_id: String,
    pub kind: String,
    pub state: ReceiptState,
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lease_acquired: Option<bool>,
    pub anchor: Option<Value>,
    pub blocked: Option<BlockedReceipt>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background_tasks_pending: Option<BackgroundTasksReminder>,
    pub terminal: Option<TerminalOutcome>,
}

pub(in crate::terminal) struct RunHandle {
    pre_start_cancelled: AtomicBool,
    interrupt_confirmed: AtomicBool,
    interrupt_pending: AtomicBool,
    bg_only_waiting: AtomicBool,
    bg_only_cancel_requested: AtomicBool,
}

impl RunHandle {
    pub(in crate::terminal) fn new() -> Self {
        Self {
            pre_start_cancelled: AtomicBool::new(false),
            interrupt_confirmed: AtomicBool::new(false),
            interrupt_pending: AtomicBool::new(false),
            bg_only_waiting: AtomicBool::new(false),
            bg_only_cancel_requested: AtomicBool::new(false),
        }
    }

    pub(in crate::terminal) fn set_interrupt_pending(&self, pending: bool) {
        self.interrupt_pending.store(pending, Ordering::SeqCst);
    }

    /// True while an Esc interruption is undecided; turn monitors must not settle then.
    pub(in crate::terminal) fn interrupt_pending(&self) -> bool {
        self.interrupt_pending.load(Ordering::SeqCst)
    }

    /// Latches a cancel received before the turn started; never reset once set.
    pub(in crate::terminal) fn request_pre_start_cancel(&self) {
        self.pre_start_cancelled.store(true, Ordering::SeqCst);
    }

    pub(in crate::terminal) fn is_pre_start_cancelled(&self) -> bool {
        self.pre_start_cancelled.load(Ordering::SeqCst)
    }

    /// Marks an in-flight turn's interruption as confirmed by the CLI/TUI.
    pub(in crate::terminal) fn confirm_interrupt(&self) {
        self.interrupt_confirmed.store(true, Ordering::SeqCst);
    }

    pub(in crate::terminal) fn is_interrupt_confirmed(&self) -> bool {
        self.interrupt_confirmed.load(Ordering::SeqCst)
    }

    /// True while the turn already wrote a reply and is waiting exclusively on background tasks.
    pub(in crate::terminal) fn set_bg_only_waiting(&self, waiting: bool) {
        self.bg_only_waiting.store(waiting, Ordering::SeqCst);
    }

    pub(in crate::terminal) fn is_bg_only_waiting(&self) -> bool {
        self.bg_only_waiting.load(Ordering::SeqCst)
    }

    /// Latches a cancel requested while only background tasks remain (no keys sent to terminal).
    pub(in crate::terminal) fn request_bg_only_cancel(&self) {
        self.bg_only_cancel_requested.store(true, Ordering::SeqCst);
    }

    pub(in crate::terminal) fn is_bg_only_cancel_requested(&self) -> bool {
        self.bg_only_cancel_requested.load(Ordering::SeqCst)
    }
}
