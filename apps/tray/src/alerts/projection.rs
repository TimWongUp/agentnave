use serde_json::Value;

use super::{Observation, Outcome};
use crate::data::Snapshot;
use crate::stage::timestamp;

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or_default()
}

fn array<'a>(value: &'a Value, key: &str) -> impl DoubleEndedIterator<Item = &'a Value> {
    value[key].as_array().into_iter().flatten()
}

pub(crate) fn observations(snapshot: &Snapshot, rounds: &[Value]) -> Vec<Observation> {
    let view = &snapshot.view["director"];
    let mut events = Vec::new();
    if snapshot.item.mode == "task" {
        for task in array(view, "tasks") {
            let outcome = match text(task, "status") {
                "succeeded" => Outcome::Completed,
                "failed" => Outcome::Failed,
                "blocked" => Outcome::Blocked,
                _ => continue,
            };
            events.push(Observation {
                key: format!("{}:task:{}:{outcome:?}", snapshot.item.id, text(task, "id")),
                title: snapshot.item.title.clone(),
                meeting: false,
                problem: outcome != Outcome::Completed,
                unstamped_problem: false,
                outcome,
                elapsed_ms: task["elapsed_ms"].as_u64(),
            });
        }
    } else if snapshot.item.mode == "show" {
        let latest = &view["round"];
        for round in rounds
            .iter()
            .filter(|r| r["number"] != latest["number"])
            .chain(latest.as_object().map(|_| latest))
        {
            meeting(snapshot, round, &mut events);
        }
    }
    events
}

fn terminal(status: &str) -> bool {
    matches!(
        status,
        "succeeded" | "failed" | "blocked" | "cancelled" | "interrupted"
    )
}

/// Start at the boss's task entry, end at the last returned seat, excluding later host
/// remarks. Serial review pauses with seats still unstarted are not a round's end.
fn meeting(snapshot: &Snapshot, round: &Value, events: &mut Vec<Observation>) {
    let view = &snapshot.view["director"];
    let number = round["number"].as_u64().unwrap_or_default();
    let key = format!("{}:round:{number}", snapshot.item.id);
    let start = array(view, "messages")
        .find(|entry| entry["round"].as_u64() == Some(number) && entry["kind"] == "task")
        .and_then(|entry| timestamp(text(entry, "time")));
    let seats: Vec<_> = array(round, "seats").collect();
    if seats.is_empty() {
        return;
    }
    for seat in &seats {
        let outcome = match text(seat, "status") {
            "failed" => Outcome::Failed,
            "blocked" => Outcome::Blocked,
            _ => continue,
        };
        let finished = seat_end(view, seat, number);
        let end = finished.or_else(|| timestamp(&snapshot.item.updated));
        events.push(Observation {
            // A retry clears `finished` and saves a new time for its next result,
            // even when it starts and finishes between two App polls.
            key: format!(
                "{key}:seat:{}:{outcome:?}:{}",
                text(seat, "seat_id"),
                finished.map(|at| at.to_string()).unwrap_or_default()
            ),
            title: snapshot.item.title.clone(),
            meeting: true,
            problem: true,
            unstamped_problem: finished.is_none(),
            outcome,
            elapsed_ms: elapsed(start, end),
        });
    }
    let Some(outcome) = round_outcome(round, &seats) else {
        return;
    };
    let end = seats
        .iter()
        .filter_map(|s| seat_end(view, s, number))
        .max()
        .or_else(|| timestamp(&snapshot.item.updated));
    events.push(Observation {
        key,
        title: snapshot.item.title.clone(),
        meeting: true,
        problem: false,
        unstamped_problem: false,
        outcome,
        elapsed_ms: elapsed(start, end),
    });
}

fn round_outcome(round: &Value, seats: &[&Value]) -> Option<Outcome> {
    if matches!(text(round, "status"), "cancelled" | "interrupted")
        || seats
            .iter()
            .any(|s| matches!(text(s, "status"), "cancelled" | "interrupted"))
    {
        return None;
    }
    let reviewing = round["status"] == "review" || seats.iter().any(|s| s["review"] == "pending");
    let finished = seats.iter().all(|s| terminal(text(s, "status")))
        || (matches!(text(round, "status"), "finished" | "failed")
            && seats
                .iter()
                .all(|s| terminal(text(s, "status")) || s["status"] == "not_started"));
    if !finished {
        return None;
    }
    let outcome = if reviewing {
        Outcome::Review
    } else if seats
        .iter()
        .any(|s| matches!(text(s, "status"), "failed" | "blocked"))
    {
        Outcome::RoundEnded
    } else {
        Outcome::Completed
    };
    Some(outcome)
}

fn elapsed(start: Option<i64>, end: Option<i64>) -> Option<u64> {
    u64::try_from(end?.checked_sub(start?)?)
        .ok()
        .map(|us| us / 1_000)
}

fn seat_end(view: &Value, seat: &Value, round: u64) -> Option<i64> {
    timestamp(text(seat, "finished")).or_else(|| {
        array(view, "messages")
            .filter(|entry| {
                entry["round"].as_u64() == Some(round)
                    && entry["speaker"] == seat["seat_id"]
                    && matches!(text(entry, "kind"), "answer" | "absent")
            })
            .filter_map(|entry| timestamp(text(entry, "time")))
            .max()
    })
}
