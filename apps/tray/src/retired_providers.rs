//! App files may still name a CLI AgentNave no longer supports (ADR-0119). Reading drops those
//! entries, as the service does, so they never keep the App from starting; a seat whose picks
//! were all retired is dropped too. Saved history is not touched here.
use serde_json::Value;

pub const RETIRED: [&str; 1] = ["devin"];

fn retired(pick: &Value) -> bool {
    pick["provider"]
        .as_str()
        .is_some_and(|provider| RETIRED.contains(&provider))
}

fn drop_keys(value: &mut Value) {
    if let Some(map) = value.as_object_mut() {
        map.retain(|key, _| !RETIRED.contains(&key.as_str()));
    }
}

/// Removes the seat's retired picks, the next remaining pick taking the primary's place; false
/// when none remains.
fn keep_seat(seat: &mut Value) -> bool {
    let Some(fields) = seat.as_object() else {
        return true;
    };
    let mut picks = vec![seat.clone()];
    if let Some(fallbacks) = fields.get("fallbacks").and_then(Value::as_array) {
        picks.extend(fallbacks.iter().cloned());
    }
    let total = picks.len();
    picks.retain(|pick| !retired(pick));
    if picks.len() == total {
        return true;
    }
    if picks.is_empty() {
        return false;
    }
    let head = picks.remove(0);
    let fields = seat.as_object_mut().expect("checked above");
    for key in ["provider", "model", "effort", "account"] {
        match head.get(key) {
            Some(value) => fields.insert(key.into(), value.clone()),
            None => fields.remove(key),
        };
    }
    fields.insert("fallbacks".into(), Value::Array(picks));
    true
}

fn seat_map(value: &mut Value) {
    if let Some(map) = value.as_object_mut() {
        map.retain(|_, seat| keep_seat(seat));
    }
}

pub fn members(document: &mut Value) {
    if let Some(clis) = document.get_mut("clis") {
        drop_keys(clis);
    }
    for section in ["modes", "defaults"] {
        if let Some(modes) = document.get_mut(section).and_then(Value::as_object_mut) {
            modes.values_mut().for_each(drop_keys);
        }
    }
}

pub fn permissions(document: &mut Value) {
    if let Some(clis) = document.get_mut("clis") {
        drop_keys(clis);
    }
}

pub fn review(document: &mut Value) {
    if let Some(seats) = document.get_mut("seats") {
        seat_map(seats);
    }
}

pub fn orchestration(document: &mut Value) {
    for group in crate::orchestration_roles::groups() {
        if let Some(seats) = document.get_mut(group) {
            seat_map(seats);
        }
    }
    if let Some(top) = document.as_object_mut()
        && top
            .get_mut("image_generation")
            .is_some_and(|seat| !keep_seat(seat))
    {
        top.remove("image_generation");
    }
    // A retired meeting seat leaves its place open, which the meeting fills when it runs.
    let seats = document
        .get_mut("meeting")
        .and_then(|meeting| meeting.get_mut("seats"));
    for seat in seats
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
        .filter(|seat| retired(seat))
    {
        *seat = Value::Null;
    }
}

pub fn decision_seats(document: &mut Value) {
    if let Some(seats) = document.get_mut("seats").and_then(Value::as_array_mut) {
        seats.retain_mut(keep_seat);
    }
}

#[cfg(test)]
mod tests;
