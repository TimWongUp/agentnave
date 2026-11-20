use super::{Event, SCANNING, UPDATED, db, now, parsers, pricing};
use crate::data::{Reader, snapshot::Snapshot};
use crate::projects::Project;
use chrono::{Days, Local, TimeZone, Timelike};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::atomic::Ordering;

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
        self.calls = self.calls.saturating_add(1);
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
            self.unpriced_calls = self.unpriced_calls.saturating_add(1);
        }
    }
    fn tokens(&self) -> i64 {
        self.fresh
            .saturating_add(self.read)
            .saturating_add(self.write5)
            .saturating_add(self.write1)
            .saturating_add(self.output)
    }
    pub(super) fn rate(&self) -> Option<f64> {
        (self.known > 0).then(|| self.known_read as f64 / self.known as f64)
    }
    fn value(&self) -> Value {
        json!({"cost_usd":self.cost,"unpriced_tokens":self.unpriced,"total_tokens":self.tokens(),"fresh_input":self.fresh,"cache_read":self.read,"cache_write":self.write5.saturating_add(self.write1),"output":self.output,"cache_hit_rate":self.rate(),"calls":self.calls})
    }
    fn overall(&self) -> Value {
        let mut v = self.value();
        v["total_calls"] = json!(self.calls);
        v["unpriced_calls"] = json!(self.unpriced_calls);
        v["cache_write_5m"] = json!(self.write5);
        v["cache_write_1h"] = json!(self.write1);
        v["cache_known_prompt"] = json!(self.known);
        v.as_object_mut().unwrap().remove("calls");
        v.as_object_mut().unwrap().remove("cache_write");
        v
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
pub(super) struct ProjectAgg {
    project: Option<Project>,
    boss: Aggregate,
    workers: Aggregate,
    providers: BTreeMap<String, (Aggregate, i64, i64)>,
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
    bosses
        .iter()
        .find(|b| b.key == key)
        .map(|b| (&b.project, false))
}

pub(super) fn add_project(
    projects: &mut BTreeMap<String, ProjectAgg>,
    project: &Project,
    worker: bool,
    e: &Event,
) {
    let p = projects.entry(project.key.clone()).or_default();
    p.project = Some(project.clone());
    if worker {
        p.workers.add(e);
    } else {
        p.boss.add(e);
    }
    let entry = p.providers.entry(e.provider.clone()).or_default();
    entry.0.add(e);
    if worker {
        entry.2 = entry.2.saturating_add(e.total());
    } else {
        entry.1 = entry.1.saturating_add(e.total());
    }
}

pub(super) fn project_value(p: ProjectAgg) -> Value {
    let mut total = p.boss.clone();
    total.cost += p.workers.cost;
    total.unpriced = total.unpriced.saturating_add(p.workers.unpriced);
    total.calls = total.calls.saturating_add(p.workers.calls);
    total.fresh = total.fresh.saturating_add(p.workers.fresh);
    total.read = total.read.saturating_add(p.workers.read);
    total.write5 = total.write5.saturating_add(p.workers.write5);
    total.write1 = total.write1.saturating_add(p.workers.write1);
    total.output = total.output.saturating_add(p.workers.output);
    total.known = total.known.saturating_add(p.workers.known);
    total.known_read = total.known_read.saturating_add(p.workers.known_read);
    let project = p.project.unwrap_or_else(Project::other);
    let providers: Vec<Value> = p
        .providers
        .into_iter()
        .map(|(name, (a, b, w))| {
            json!({
                "provider": name,
                "cost_usd": a.cost,
                "unpriced_tokens": a.unpriced,
                "total_tokens": a.tokens(),
                "boss_tokens": b,
                "worker_tokens": w
            })
        })
        .collect();
    json!({"key":project.key,"label":project.label,"path":project.path,"total":total.value(),"boss":p.boss.value(),"workers":p.workers.value(),"by_provider":providers})
}

fn dates(range: &str) -> Vec<chrono::NaiveDate> {
    let count = match range {
        "7d" => 7,
        "30d" => 30,
        _ => 1,
    };
    let today = Local::now().date_naive();
    (0..count)
        .rev()
        .filter_map(|n| today.checked_sub_days(Days::new(n)))
        .collect()
}
pub(super) fn start_in<T: TimeZone>(tz: &T, date: chrono::NaiveDate) -> i64 {
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
fn start(date: chrono::NaiveDate) -> i64 {
    start_in(&Local, date)
}

pub(super) fn bucket_keys(range: &str, days: &[chrono::NaiveDate]) -> Vec<String> {
    if range == "today" {
        (0..24).map(|hour| format!("{hour:02}:00")).collect()
    } else {
        days.iter().map(ToString::to_string).collect()
    }
}

pub(super) fn bucket_of(range: &str, ts_ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ts_ms)
        .map(|t| {
            let local = t.with_timezone(&Local);
            if range == "today" {
                format!("{:02}:00", local.hour())
            } else {
                local.date_naive().to_string()
            }
        })
        .unwrap_or_default()
}

pub(super) fn respond(reader: &Reader, range: &str) -> Result<Value, Box<dyn std::error::Error>> {
    let days = dates(range);
    let from = start(days[0]);
    let end = now() + 1;
    let events = db::events(&db::open(&crate::data_files::data_root())?, from, end)?;
    let (snapshots, projects) = reader.usage_views();
    let (windows, bosses) = attribution(&snapshots, &projects);
    let mut overall = Aggregate::default();
    let mut daily: BTreeMap<String, (Aggregate, BTreeMap<String, Aggregate>)> =
        bucket_keys(range, &days)
            .into_iter()
            .map(|key| (key, Default::default()))
            .collect();
    let mut providers: BTreeMap<String, (Aggregate, BTreeMap<String, Aggregate>)> = BTreeMap::new();
    let mut by_project = BTreeMap::new();
    for e in &events {
        overall.add(e);
        let bucket = bucket_of(range, e.ts);
        if let Some((a, p)) = daily.get_mut(&bucket) {
            a.add(e);
            p.entry(e.provider.clone()).or_default().add(e);
        }
        let (a, models) = providers.entry(e.provider.clone()).or_default();
        a.add(e);
        models.entry(e.model.clone()).or_default().add(e);
        if let Some((project, worker)) = assign(e, &windows, &bosses) {
            add_project(&mut by_project, project, worker, e);
        }
    }
    let daily: Vec<Value> = daily
        .into_iter()
        .map(|(date, (a, p))| {
            let mut v = a.value();
            v["date"] = json!(date);
            v.as_object_mut().unwrap().remove("calls");
            v["by_provider"] = json!(
                p.into_iter()
                    .map(|(k, v)| (k, json!({"cost_usd":v.cost,"total_tokens":v.tokens()})))
                    .collect::<BTreeMap<_, _>>()
            );
            v
        })
        .collect();
    let providers: Vec<Value> = providers
        .into_iter()
        .map(|(name, (a, models))| {
            let mut v = a.value();
            v["provider"] = json!(name);
            v["models"] = json!(
                models
                    .into_iter()
                    .map(|(model, a)| {
                        let mut v = a.value();
                        v["priced"] = json!(a.unpriced_calls == 0);
                        v.as_object_mut().unwrap().remove("unpriced_tokens");
                        v["model"] = json!(model);
                        v
                    })
                    .collect::<Vec<_>>()
            );
            v
        })
        .collect();
    let granularity = if range == "today" { "hour" } else { "day" };
    Ok(
        json!({"range":range,"granularity":granularity,"updated_at":UPDATED.load(Ordering::Relaxed),"scanning":SCANNING.load(Ordering::Acquire),"covered_providers":super::COVERED_PROVIDERS,"uncovered_installed":reader.usage_uncovered(),"overall":overall.overall(),"daily":daily,"providers":providers,"projects":by_project.into_values().map(project_value).collect::<Vec<_>>() }),
    )
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
    json!({"group":group,"has_data":total.calls>0,"total":total.value(),"boss":boss.value(),"workers":workers.value()})
}
