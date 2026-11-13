//! Preset rule sections, switches, templates and assembly order loaded from the shared
//! `src/agentnave/orchestration_rules.json` packaged with the Python service.
use serde_json::{Value, json};
use std::sync::LazyLock;

static CATALOG: LazyLock<Value> = LazyLock::new(|| {
    serde_json::from_str(include_str!(
        "../../../../src/agentnave/orchestration_rules.json"
    ))
    .expect("valid bundled orchestration rules")
});

pub(super) fn catalog() -> &'static Value {
    &CATALOG
}

fn str_at(node: &'static Value, key: &str) -> &'static str {
    node[key]
        .as_str()
        .unwrap_or_else(|| panic!("missing string {key} in orchestration_rules.json"))
}

pub(super) fn section(name: &str) -> &'static str {
    str_at(&catalog()["sections"], name)
}

fn flag(document: &Value, path: &str, default: bool) -> bool {
    path.split('.')
        .try_fold(document, |current, key| current.get(key))
        .and_then(Value::as_bool)
        .unwrap_or(default)
}

fn switched_section(document: &Value, switch_name: &str) -> &'static str {
    let switch = &catalog()["switches"][switch_name];
    let default = switch["default"]
        .as_bool()
        .expect("switch default in orchestration_rules.json");
    let branch = if flag(document, str_at(switch, "field"), default) {
        "when_true"
    } else {
        "when_false"
    };
    section(str_at(switch, branch))
}

fn format_meeting_seat(seat: &Value) -> Option<String> {
    let md = &catalog()["meeting_defaults"];
    let provider = seat.get("provider")?.as_str()?;
    let name = md["provider_names"][provider].as_str()?;
    Some(
        str_at(md, "seat_provider")
            .replace("{name}", name)
            .replace("{provider}", provider),
    )
}

pub(super) fn meeting_line(document: &Value) -> Option<String> {
    let md = &catalog()["meeting_defaults"];
    let meeting = document.get("meeting")?.as_object()?;
    let count = meeting.get("count")?.as_u64()?;
    let seats: Vec<String> = meeting
        .get("seats")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .take(count as usize)
                .filter_map(format_meeting_seat)
                .collect()
        })
        .unwrap_or_default();
    let key = match seats.len() {
        0 => "none_named",
        len if len == count as usize => "all_named",
        _ => "some_named",
    };
    Some(
        str_at(md, key)
            .replace("{count}", &count.to_string())
            .replace("{seats}", &seats.join(str_at(md, "separator"))),
    )
}

fn assemble_steps(document: &Value, steps: &[Value], apply_lead: bool) -> String {
    let lead = &catalog()["switches"]["lead_only"];
    let lead_default = lead["default"]
        .as_bool()
        .expect("lead_only default in orchestration_rules.json");
    let lead_on = apply_lead && flag(document, str_at(lead, "field"), lead_default);
    let mut out = String::new();
    for step in steps {
        let name = step
            .as_str()
            .expect("order step string in orchestration_rules.json");
        match name {
            "meeting_defaults" => {
                if let Some(line) = meeting_line(document) {
                    out.push_str(&line);
                }
            }
            "enabled_roles" => out.push_str(&super::roles::enabled_lines(document)),
            "image_generation" => {
                if document["image_generation"].is_object() {
                    out.push_str(section("image_generation"));
                }
            }
            "split_meetings" | "parallel_meetings" => {
                out.push_str(switched_section(document, name));
            }
            _ => {
                let text = section(name);
                if lead_on && name == str_at(lead, "replaces_in") {
                    out.push_str(
                        &text.replace(str_at(lead, "when_false"), str_at(lead, "when_true")),
                    );
                } else {
                    out.push_str(text);
                }
            }
        }
    }
    out
}

pub(super) fn preset_text(document: &Value, mode: &str) -> Option<String> {
    let steps = catalog()["order"].get(mode)?.as_array()?;
    Some(assemble_steps(document, steps, true))
}

fn parallel_role_base(document: &Value) -> String {
    let steps = catalog()["order"]["parallel"]
        .as_array()
        .expect("parallel order in orchestration_rules.json");
    let end = steps
        .iter()
        .position(|step| step.as_str() == Some("enabled_roles"))
        .unwrap_or(steps.len());
    assemble_steps(document, &steps[..end], false)
}

pub(super) fn presets_payload(document: &Value) -> Value {
    let rules = catalog();
    let md = &rules["meeting_defaults"];
    let lead = &rules["switches"]["lead_only"];
    let default_count = md["default_count"]
        .as_u64()
        .expect("meeting default_count in orchestration_rules.json");
    let shared = section("shared");
    json!({"meetings": super::text_of(document, "meetings"),
        "split": super::text_of(document, "split"),
        "parallel": super::text_of(document, "parallel"),
        "meeting": meeting_line(document),
        "meetings_parts": {"head": section("meetings_only_head"), "tail": section("meetings_only_tail")},
        "meeting_parts": {"default_count": default_count, "separator": str_at(md, "separator"),
            "seat_provider": str_at(md, "seat_provider"),
            "all": str_at(md, "all_named"), "some": str_at(md, "some_named"), "none": str_at(md, "none_named")},
        "image_generation": section("image_generation"),
        "role_sections": super::roles::sections(),
        "role_base": {"parallel": parallel_role_base(document)},
        "parallel_base": format!("{shared}{}", section("parallel_writes")),
        "split_parts": {"base": format!("{shared}{}", section("split_parallel_writes")),
            "meetings_asked": section("meetings_when_asked"),
            "meetings_own": section("meetings_on_tradeoffs")},
        "lead": {"asked": str_at(lead, "when_false"), "always": str_at(lead, "when_true")}})
}
