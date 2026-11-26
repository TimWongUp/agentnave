use super::{Event, SCANNING, UPDATED, buckets, db, now, parsers, pricing};
use crate::data::{Reader, snapshot::Snapshot};
use crate::projects::{OTHER, Project};
use chrono::{Days, Local, NaiveDate, TimeZone, Timelike};
use rusqlite::Connection;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::atomic::Ordering;

use buckets::{BucketRow, DAY, HOUR};

#[derive(Default, Clone)]
pub(super) struct Aggregate {
    cost: f64,
    unpriced: i64,
    unpriced_calls: i64,
    calls: i64,
    fresh: i64,
    read: i64,
    write5: i64,
    write1: i64,
    output: i64,
    known: i64,
    known_read: i64,
}

impl Aggregate {
    pub(super) fn add(&mut self, e: &Event) {
        let calls = e.calls.max(1);
        self.calls = self.calls.saturating_add(calls);
        self.fresh = self.fresh.saturating_add(e.fresh);
        self.read = self.read.saturating_add(e.read);
        self.write5 = self.write5.saturating_add(e.write5);
        self.write1 = self.write1.saturating_add(e.write1);
        self.output = self.output.saturating_add(e.output);
        if !e.unknown {
            self.known = self.known.saturating_add(e.prompt());
            self.known_read = self.known_read.saturating_add(e.read);
        }
        if let Some(cost) = pricing::cost(e) {
            self.cost += cost;
        } else {
            self.unpriced = self.unpriced.saturating_add(e.total());
            self.unpriced_calls = self.unpriced_calls.saturating_add(calls);
        }
    }

    fn add_bucket(&mut self, row: &BucketRow) {
        self.calls = self.calls.saturating_add(row.calls);
        self.fresh = self.fresh.saturating_add(row.fresh_input);
        self.read = self.read.saturating_add(row.cache_read);
        self.write5 = self.write5.saturating_add(row.cache_write_5m);
        self.write1 = self.write1.saturating_add(row.cache_write_1h);
        self.output = self.output.saturating_add(row.output);
        self.known = self.known.saturating_add(row.cache_known_prompt);
        self.known_read = self.known_read.saturating_add(row.cache_known_read);
        self.cost += row.cost_usd;
        self.unpriced = self.unpriced.saturating_add(row.unpriced_tokens);
        self.unpriced_calls = self.unpriced_calls.saturating_add(row.unpriced_calls);
    }

    fn add_agg(&mut self, other: &Self) {
        self.cost += other.cost;
        self.unpriced = self.unpriced.saturating_add(other.unpriced);
        self.unpriced_calls = self.unpriced_calls.saturating_add(other.unpriced_calls);
        self.calls = self.calls.saturating_add(other.calls);
        self.fresh = self.fresh.saturating_add(other.fresh);
        self.read = self.read.saturating_add(other.read);
        self.write5 = self.write5.saturating_add(other.write5);
        self.write1 = self.write1.saturating_add(other.write1);
        self.output = self.output.saturating_add(other.output);
        self.known = self.known.saturating_add(other.known);
        self.known_read = self.known_read.saturating_add(other.known_read);
    }

    fn tokens(&self) -> i64 {
        self.fresh
            .saturating_add(self.read)
            .saturating_add(self.write5)
            .saturating_add(self.write1)
            .saturating_add(self.output)
    }

    fn net_tokens(&self) -> i64 {
        self.fresh
            .saturating_add(self.write5)
            .saturating_add(self.write1)
            .saturating_add(self.output)
    }

    fn cache_write(&self) -> i64 {
        self.write5.saturating_add(self.write1)
    }

    pub(super) fn rate(&self) -> Option<f64> {
        (self.known > 0).then(|| self.known_read as f64 / self.known as f64)
    }

    fn value(&self) -> Value {
        json!({
            "cost_usd": self.cost,
            "unpriced_tokens": self.unpriced,
            "total_tokens": self.tokens(),
            "net_tokens": self.net_tokens(),
            "fresh_input": self.fresh,
            "cache_read": self.read,
            "cache_write": self.cache_write(),
            "output": self.output,
            "cache_hit_rate": self.rate(),
            "calls": self.calls,
        })
    }

    fn overall(&self) -> Value {
        json!({
            "cost_usd": self.cost,
            "unpriced_tokens": self.unpriced,
            "total_tokens": self.tokens(),
            "net_tokens": self.net_tokens(),
            "fresh_input": self.fresh,
            "cache_read": self.read,
            "cache_write_5m": self.write5,
            "cache_write_1h": self.write1,
            "output": self.output,
            "cache_hit_rate": self.rate(),
            "total_calls": self.calls,
            "unpriced_calls": self.unpriced_calls,
            "cache_known_prompt": self.known,
        })
    }

    fn prev_overall(&self) -> Value {
        json!({
            "cost_usd": self.cost,
            "unpriced_tokens": self.unpriced,
            "total_tokens": self.tokens(),
            "net_tokens": self.net_tokens(),
            "fresh_input": self.fresh,
            "cache_read": self.read,
            "cache_write": self.cache_write(),
            "output": self.output,
            "cache_hit_rate": self.rate(),
            "total_calls": self.calls,
        })
    }
}

pub(super) struct Window {
    thread: String,
    start: i64,
    end: i64,
    project: Project,
}
pub(super) struct Boss {
    key: String,
    project: Project,
}

#[derive(Default)]
struct ProjectProviderAgg {
    total: Aggregate,
    boss_tokens: i64,
    worker_tokens: i64,
    native_tokens: i64,
    boss_cost: f64,
    worker_cost: f64,
    native_cost: f64,
}

#[derive(Default)]
pub(super) struct ProjectAgg {
    project: Option<Project>,
    boss: Aggregate,
    workers: Aggregate,
    native: Aggregate,
    providers: BTreeMap<String, ProjectProviderAgg>,
}

pub(super) fn boss_key(provider: &str, session: &str) -> String {
    let host = serde_json::to_string(provider).unwrap_or_default();
    let id = serde_json::to_string(session).unwrap_or_default();
    format!("s-{}", &parsers::hash(&format!("[{host}, {id}]"))[..32])
}

fn at(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|t| t.timestamp_millis())
}

pub(super) fn attribution<'a>(
    snapshots: impl IntoIterator<Item = &'a Snapshot>,
    projects: &BTreeMap<String, Project>,
) -> (Vec<Window>, Vec<Boss>) {
    let mut windows = Vec::new();
    let mut bosses = Vec::new();
    for s in snapshots {
        let Some(group) = crate::stage::chat_group(&s.item) else {
            continue;
        };
        let project = projects.get(&group).cloned().unwrap_or_else(Project::other);
        bosses.push(Boss {
            key: group,
            project: project.clone(),
        });
        if s.item.mode != "task" {
            continue;
        }
        for task in s.view["director"]["tasks"].as_array().into_iter().flatten() {
            let (Some(thread), Some(start)) =
                (task["thread"].as_str(), task["time"].as_str().and_then(at))
            else {
                continue;
            };
            let end = if task["status"] == "running" {
                now()
            } else {
                start
                    .saturating_add(task["elapsed_ms"].as_i64().unwrap_or(0))
                    .saturating_add(2000)
            };
            windows.push(Window {
                thread: thread.to_owned(),
                start,
                end,
                project: project.clone(),
            });
        }
    }
    windows.sort_by(|a, b| {
        a.start
            .cmp(&b.start)
            .then_with(|| a.project.key.cmp(&b.project.key))
    });
    (windows, bosses)
}

pub(super) fn assign<'a>(
    e: &Event,
    windows: &'a [Window],
    bosses: &'a [Boss],
) -> Option<(&'a Project, bool)> {
    if let Some(w) = windows
        .iter()
        .find(|w| w.thread == e.thread && e.ts >= w.start && e.ts <= w.end)
    {
        return Some((&w.project, true));
    }
    let key = boss_key(&e.provider, &e.root);
    if let Some(b) = bosses.iter().find(|b| b.key == key) {
        return Some((&b.project, false));
    }
    if e.provider == "claude-desktop" {
        let fallback = boss_key("claude", &e.root);
        if let Some(b) = bosses.iter().find(|b| b.key == fallback) {
            return Some((&b.project, false));
        }
    }
    None
}

#[cfg(test)]
pub(super) fn add_project(
    projects: &mut BTreeMap<String, ProjectAgg>,
    project: &Project,
    worker: bool,
    e: &Event,
) {
    let p = projects.entry(project.key.clone()).or_default();
    p.project = Some(project.clone());
    let cost = pricing::cost(e).unwrap_or(0.0);
    let tokens = e.total();
    let entry = p.providers.entry(e.provider.clone()).or_default();
    entry.total.add(e);
    if worker {
        p.workers.add(e);
        entry.worker_tokens = entry.worker_tokens.saturating_add(tokens);
        entry.worker_cost += cost;
    } else {
        p.boss.add(e);
        entry.boss_tokens = entry.boss_tokens.saturating_add(tokens);
        entry.boss_cost += cost;
    }
}

fn add_project_bucket(
    projects: &mut BTreeMap<String, ProjectAgg>,
    catalog: &BTreeMap<String, Project>,
    row: &BucketRow,
) {
    let p = projects.entry(row.project_key.clone()).or_default();
    if p.project.is_none() {
        p.project = Some(if row.project_key == OTHER {
            Project::other()
        } else {
            catalog.get(&row.project_key).cloned().unwrap_or_else(|| {
                let label = std::path::Path::new(&row.project_key)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| row.project_key.clone());
                Project {
                    key: row.project_key.clone(),
                    label,
                    path: row.project_key.clone(),
                }
            })
        });
    }
    let tokens = row
        .fresh_input
        .saturating_add(row.cache_read)
        .saturating_add(row.cache_write_5m)
        .saturating_add(row.cache_write_1h)
        .saturating_add(row.output);
    let entry = p.providers.entry(row.provider.clone()).or_default();
    entry.total.add_bucket(row);
    match row.role_kind.as_str() {
        "worker" => {
            p.workers.add_bucket(row);
            entry.worker_tokens = entry.worker_tokens.saturating_add(tokens);
            entry.worker_cost += row.cost_usd;
        }
        "boss" => {
            p.boss.add_bucket(row);
            entry.boss_tokens = entry.boss_tokens.saturating_add(tokens);
            entry.boss_cost += row.cost_usd;
        }
        _ => {
            p.native.add_bucket(row);
            entry.native_tokens = entry.native_tokens.saturating_add(tokens);
            entry.native_cost += row.cost_usd;
        }
    }
}

pub(super) fn project_value(p: ProjectAgg) -> Value {
    let mut total = p.boss.clone();
    total.add_agg(&p.workers);
    total.add_agg(&p.native);
    let project = p.project.unwrap_or_else(Project::other);
    let providers: Vec<Value> = p
        .providers
        .into_iter()
        .map(|(name, a)| {
            json!({
                "provider": name,
                "cost_usd": a.total.cost,
                "unpriced_tokens": a.total.unpriced,
                "total_tokens": a.total.tokens(),
                "net_tokens": a.total.net_tokens(),
                "boss_tokens": a.boss_tokens,
                "worker_tokens": a.worker_tokens,
                "native_tokens": a.native_tokens,
                "boss_cost": a.boss_cost,
                "worker_cost": a.worker_cost,
                "native_cost": a.native_cost,
            })
        })
        .collect();
    json!({
        "key": project.key,
        "label": project.label,
        "path": project.path,
        "total": total.value(),
        "boss": p.boss.value(),
        "workers": p.workers.value(),
        "native": p.native.value(),
        "by_provider": providers,
    })
}

pub(super) fn start_in<T: TimeZone>(tz: &T, date: NaiveDate) -> i64 {
    (0..24)
        .filter_map(|hour| date.and_hms_opt(hour, 0, 0))
        .find_map(|naive| tz.from_local_datetime(&naive).earliest())
        .map(|d| d.timestamp_millis())
        .unwrap_or_else(|| {
            date.and_hms_opt(0, 0, 0)
                .unwrap()
                .and_utc()
                .timestamp_millis()
        })
}

fn local_date(ts_ms: i64) -> NaiveDate {
    chrono::DateTime::from_timestamp_millis(ts_ms)
        .map(|t| t.with_timezone(&Local).date_naive())
        .unwrap_or_else(|| Local::now().date_naive())
}

fn format_local_hour(ts_ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ts_ms)
        .map(|t| {
            let local = t.with_timezone(&Local);
            format!("{}T{:02}:00", local.date_naive(), local.hour())
        })
        .unwrap_or_default()
}

pub(super) struct QuerySpec<'a> {
    pub range: &'a str,
    pub custom: Option<(i64, i64)>,
    pub granularity: Option<&'a str>,
}

struct WindowPlan {
    from_ms: i64,
    to_ms: i64,
    prev_from: i64,
    prev_to: i64,
    granularity: &'static str,
    available: Vec<&'static str>,
}

pub(super) fn available_granularities(span_ms: i64) -> Vec<&'static str> {
    let mut out = Vec::new();
    // Allow 1h DST slack so 7-day or 2-day local calendar spans across transitions stay valid.
    if span_ms <= 7 * DAY + HOUR {
        out.push("hour");
    }
    if (2 * DAY - HOUR..=92 * DAY + HOUR).contains(&span_ms) {
        out.push("day");
    }
    if span_ms >= 14 * DAY - HOUR {
        out.push("week");
    }
    if out.is_empty() {
        out.push("hour");
    }
    out
}

pub(super) fn default_granularity(span_ms: i64) -> &'static str {
    if span_ms <= 48 * HOUR {
        "hour"
    } else if span_ms <= 60 * DAY + HOUR {
        "day"
    } else {
        "week"
    }
}

fn plan_window(spec: &QuerySpec<'_>, now_ms: i64) -> WindowPlan {
    let current_hour = buckets::hour_ts_utc(now_ms);
    let today = local_date(now_ms);
    let tomorrow = today.succ_opt().unwrap_or(today);
    let (from_ms, to_ms, prev_from, prev_to) = match spec.range {
        "24h" => {
            let to_ms = current_hour + HOUR;
            let from_ms = current_hour - 23 * HOUR;
            (from_ms, to_ms, from_ms - 24 * HOUR, from_ms)
        }
        "today" => {
            let from_ms = start_in(&Local, today);
            let to_ms = start_in(&Local, tomorrow);
            let yesterday = today.pred_opt().unwrap_or(today);
            let prev_from = start_in(&Local, yesterday);
            let active_span = (current_hour + HOUR - from_ms).clamp(HOUR, to_ms - from_ms);
            let prev_to = (prev_from + active_span).min(from_ms);
            (from_ms, to_ms, prev_from, prev_to)
        }
        "7d" | "30d" | "90d" => {
            let days = match spec.range {
                "7d" => 7u64,
                "30d" => 30u64,
                _ => 90u64,
            };
            let first = today.checked_sub_days(Days::new(days - 1)).unwrap_or(today);
            let from_ms = start_in(&Local, first);
            let to_ms = start_in(&Local, tomorrow);
            let span = to_ms - from_ms;
            (from_ms, to_ms, from_ms - span, from_ms)
        }
        _ => {
            let (from_ms, to_ms) = spec
                .custom
                .unwrap_or((current_hour - 23 * HOUR, current_hour + HOUR));
            let span = to_ms - from_ms;
            (from_ms, to_ms, from_ms - span, from_ms)
        }
    };
    let span = to_ms.saturating_sub(from_ms);
    let available = available_granularities(span);
    let chosen = spec
        .granularity
        .and_then(|g| available.iter().copied().find(|&a| a == g))
        .unwrap_or_else(|| default_granularity(span));
    WindowPlan {
        from_ms,
        to_ms,
        prev_from,
        prev_to,
        granularity: chosen,
        available,
    }
}

struct Slot {
    date: String,
    start_ms: i64,
    end_ms: i64,
    future: bool,
    total: Aggregate,
    by_provider: BTreeMap<String, Aggregate>,
    by_model: BTreeMap<(String, String), Aggregate>,
}

fn build_slots(from_ms: i64, to_ms: i64, now_ms: i64, granularity: &str) -> Vec<Slot> {
    let current_hour = buckets::hour_ts_utc(now_ms);
    let mut slots = Vec::new();
    match granularity {
        "hour" => {
            let mut cursor = from_ms;
            while cursor < to_ms {
                let end = (cursor + HOUR).min(to_ms);
                slots.push(Slot {
                    date: format_local_hour(cursor),
                    start_ms: cursor,
                    end_ms: end,
                    future: cursor > current_hour,
                    total: Aggregate::default(),
                    by_provider: BTreeMap::new(),
                    by_model: BTreeMap::new(),
                });
                cursor = end;
            }
        }
        "day" => {
            let mut day = local_date(from_ms);
            loop {
                let day_start = start_in(&Local, day);
                if day_start >= to_ms {
                    break;
                }
                let next_day = day.succ_opt().unwrap_or(day);
                let next_start = start_in(&Local, next_day);
                let start = day_start.max(from_ms);
                let end = next_start.min(to_ms);
                if start < end {
                    slots.push(Slot {
                        date: day.to_string(),
                        start_ms: start,
                        end_ms: end,
                        future: start > now_ms,
                        total: Aggregate::default(),
                        by_provider: BTreeMap::new(),
                        by_model: BTreeMap::new(),
                    });
                }
                if next_day == day {
                    break;
                }
                day = next_day;
            }
        }
        _ => {
            // "week": 7-day local calendar slots anchored at the first date in the window.
            let mut day = local_date(from_ms);
            loop {
                let slot_start = start_in(&Local, day);
                if slot_start >= to_ms {
                    break;
                }
                let next_day = day.checked_add_days(Days::new(7)).unwrap_or(day);
                let next_start = start_in(&Local, next_day);
                let start = slot_start.max(from_ms);
                let end = next_start.min(to_ms);
                if start < end {
                    slots.push(Slot {
                        date: day.to_string(),
                        start_ms: start,
                        end_ms: end,
                        future: start > now_ms,
                        total: Aggregate::default(),
                        by_provider: BTreeMap::new(),
                        by_model: BTreeMap::new(),
                    });
                }
                if next_day == day {
                    break;
                }
                day = next_day;
            }
        }
    }
    slots
}

fn slot_value(slot: Slot) -> Value {
    let by_provider: BTreeMap<String, Value> = slot
        .by_provider
        .into_iter()
        .map(|(k, v)| {
            (
                k,
                json!({
                    "cost_usd": v.cost,
                    "total_tokens": v.tokens(),
                    "net_tokens": v.net_tokens(),
                }),
            )
        })
        .collect();
    let by_model: BTreeMap<String, Value> = slot
        .by_model
        .into_iter()
        .map(|((provider, model), v)| {
            let key = format!("{provider}::{model}");
            (
                key,
                json!({
                    "provider": provider,
                    "model": model,
                    "cost_usd": v.cost,
                    "total_tokens": v.tokens(),
                    "net_tokens": v.net_tokens(),
                }),
            )
        })
        .collect();
    json!({
        "date": slot.date,
        "start_ms": slot.start_ms,
        "end_ms": slot.end_ms,
        "future": slot.future,
        "cost_usd": slot.total.cost,
        "unpriced_tokens": slot.total.unpriced,
        "total_tokens": slot.total.tokens(),
        "net_tokens": slot.total.net_tokens(),
        "fresh_input": slot.total.fresh,
        "cache_read": slot.total.read,
        "cache_write": slot.total.cache_write(),
        "output": slot.total.output,
        "cache_hit_rate": slot.total.rate(),
        "by_provider": by_provider,
        "by_model": by_model,
    })
}

fn prev_fields(has_prev: bool, agg: Option<&Aggregate>) -> (Value, Value, Value) {
    if !has_prev {
        return (Value::Null, Value::Null, Value::Null);
    }
    match agg {
        Some(a) => (json!(a.cost), json!(a.tokens()), json!(a.net_tokens())),
        None => (json!(0.0), json!(0), json!(0)),
    }
}

fn build_providers(
    current: BTreeMap<String, (Aggregate, BTreeMap<String, Aggregate>)>,
    prev: &BTreeMap<String, (Aggregate, BTreeMap<String, Aggregate>)>,
    has_prev: bool,
) -> Vec<Value> {
    current
        .into_iter()
        .map(|(name, (a, models))| {
            let prev_p = prev.get(&name);
            let (pc, pt, pn) = prev_fields(has_prev, prev_p.map(|(pa, _)| pa));
            let model_list: Vec<Value> = models
                .into_iter()
                .map(|(model, ma)| {
                    let prev_m = prev_p.and_then(|(_, pm)| pm.get(&model));
                    let (mc, mt, mn) = prev_fields(has_prev, prev_m);
                    json!({
                        "key": format!("{name}::{model}"),
                        "provider": name,
                        "model": model,
                        "priced": ma.unpriced_calls == 0,
                        "cost_usd": ma.cost,
                        "prev_cost_usd": mc,
                        "unpriced_tokens": ma.unpriced,
                        "total_tokens": ma.tokens(),
                        "prev_total_tokens": mt,
                        "net_tokens": ma.net_tokens(),
                        "prev_net_tokens": mn,
                        "fresh_input": ma.fresh,
                        "cache_read": ma.read,
                        "cache_write": ma.cache_write(),
                        "output": ma.output,
                        "cache_known_prompt": ma.known,
                        "cache_known_read": ma.known_read,
                        "cache_hit_rate": ma.rate(),
                        "calls": ma.calls,
                    })
                })
                .collect();
            json!({
                "provider": name,
                "cost_usd": a.cost,
                "prev_cost_usd": pc,
                "unpriced_tokens": a.unpriced,
                "total_tokens": a.tokens(),
                "prev_total_tokens": pt,
                "net_tokens": a.net_tokens(),
                "prev_net_tokens": pn,
                "fresh_input": a.fresh,
                "cache_read": a.read,
                "cache_write": a.cache_write(),
                "output": a.output,
                "cache_hit_rate": a.rate(),
                "calls": a.calls,
                "models": model_list,
            })
        })
        .collect()
}

/// Pure DB query implementation (reads ONLY `usage_hourly_buckets` and `usage_projects`).
pub(super) fn respond_db(
    conn: &Connection,
    spec: &QuerySpec<'_>,
    now_ms: i64,
    uncovered_installed: Vec<String>,
) -> rusqlite::Result<Value> {
    let plan = plan_window(spec, now_ms);
    let min_ts = buckets::min_hour_ts(conn)?;
    let rows = buckets::query_buckets(conn, plan.from_ms, plan.to_ms)?;
    let prev_rows = buckets::query_buckets(conn, plan.prev_from, plan.prev_to)?;
    let catalog = buckets::query_projects(conn)?;

    let mut overall = Aggregate::default();
    let mut slots = build_slots(plan.from_ms, plan.to_ms, now_ms, plan.granularity);
    let mut providers: BTreeMap<String, (Aggregate, BTreeMap<String, Aggregate>)> = BTreeMap::new();
    let mut by_project: BTreeMap<String, ProjectAgg> = BTreeMap::new();

    for row in &rows {
        overall.add_bucket(row);
        let idx = slots.partition_point(|s| s.end_ms <= row.hour_ts_utc);
        if let Some(slot) = slots.get_mut(idx)
            && row.hour_ts_utc >= slot.start_ms
            && row.hour_ts_utc < slot.end_ms
        {
            slot.total.add_bucket(row);
            slot.by_provider
                .entry(row.provider.clone())
                .or_default()
                .add_bucket(row);
            slot.by_model
                .entry((row.provider.clone(), row.model.clone()))
                .or_default()
                .add_bucket(row);
        }
        let (pa, models) = providers.entry(row.provider.clone()).or_default();
        pa.add_bucket(row);
        models.entry(row.model.clone()).or_default().add_bucket(row);
        add_project_bucket(&mut by_project, &catalog, row);
    }

    let mut prev_overall_agg = Aggregate::default();
    let mut prev_providers: BTreeMap<String, (Aggregate, BTreeMap<String, Aggregate>)> =
        BTreeMap::new();
    for row in &prev_rows {
        prev_overall_agg.add_bucket(row);
        let (pa, models) = prev_providers.entry(row.provider.clone()).or_default();
        pa.add_bucket(row);
        models.entry(row.model.clone()).or_default().add_bucket(row);
    }

    let has_prev = match min_ts {
        None => false,
        Some(min) if plan.prev_to <= min => false,
        Some(min) if prev_overall_agg.calls == 0 && plan.prev_from < min => false,
        Some(_) => true,
    };

    let daily: Vec<Value> = slots.into_iter().map(slot_value).collect();
    let providers_json = build_providers(providers, &prev_providers, has_prev);
    let prev_by_model: Value = if has_prev {
        let list: Vec<Value> = prev_providers
            .iter()
            .flat_map(|(prov, (_, models))| {
                models.iter().map(move |(model, ma)| {
                    json!({
                        "provider": prov,
                        "model": model,
                        "cost_usd": ma.cost,
                        "total_tokens": ma.tokens(),
                        "net_tokens": ma.net_tokens(),
                    })
                })
            })
            .collect();
        Value::Array(list)
    } else {
        Value::Null
    };
    let projects_json: Vec<Value> = by_project.into_values().map(project_value).collect();
    let prev_overall = if has_prev {
        prev_overall_agg.prev_overall()
    } else {
        Value::Null
    };

    Ok(json!({
        "range": spec.range,
        "from_ms": plan.from_ms,
        "to_ms": plan.to_ms,
        "now_ms": now_ms,
        "granularity": plan.granularity,
        "available_granularities": plan.available,
        "min_ts_ms": min_ts,
        "updated_at": UPDATED.load(Ordering::Relaxed),
        "scanning": SCANNING.load(Ordering::Acquire),
        "covered_providers": super::COVERED_PROVIDERS,
        "uncovered_installed": uncovered_installed,
        "overall": overall.overall(),
        "prev_overall": prev_overall,
        "prev_by_model": prev_by_model,
        "daily": daily,
        "providers": providers_json,
        "projects": projects_json,
    }))
}

pub(super) fn respond(
    reader: &Reader,
    spec: &QuerySpec<'_>,
) -> Result<Value, Box<dyn std::error::Error>> {
    let conn = db::open(&crate::data_files::data_root())?;
    Ok(respond_db(&conn, spec, now(), reader.usage_uncovered())?)
}

pub(super) fn workspace(reader: &Reader, group: &str) -> Result<Value, Box<dyn std::error::Error>> {
    let events = db::events(&db::open(&crate::data_files::data_root())?, 0, i64::MAX)?;
    Ok(workspace_of(&reader.usage_views().0, &events, group))
}

pub(super) fn workspace_of(snapshots: &[Snapshot], events: &[Event], group: &str) -> Value {
    let (windows, bosses) = attribution(
        snapshots
            .iter()
            .filter(|s| crate::stage::chat_group(&s.item).as_deref() == Some(group)),
        &BTreeMap::new(),
    );
    let mut total = Aggregate::default();
    let mut boss = Aggregate::default();
    let mut workers = Aggregate::default();
    for e in events {
        match assign(e, &windows, &bosses) {
            Some((_, true)) => {
                workers.add(e);
                total.add(e);
            }
            Some((_, false)) => {
                boss.add(e);
                total.add(e);
            }
            None => {}
        }
    }
    json!({
        "group": group,
        "has_data": total.calls > 0,
        "total": total.value(),
        "boss": boss.value(),
        "workers": workers.value(),
    })
}
