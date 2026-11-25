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

/// Whether the lead-only line is swapped in the section `name`.
fn replaces_in(lead: &'static Value, name: &str) -> bool {
    lead["replaces_in"]
        .as_array()
        .expect("lead_only replaces_in in orchestration_rules.json")
        .iter()
        .any(|section| section.as_str() == Some(name))
}

fn assemble_steps(
    document: &Value,
    steps: &[Value],
    apply_lead: bool,
    with_t3_sections: bool,
) -> String {
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
            "dispatch_split" | "dispatch_decision" => {
                if with_t3_sections {
                    out.push_str(section("t3_non_t3_heading"));
                }
                out.push_str(section(name));
            }
            name if catalog()["switches"].get(name).is_some() => {
                out.push_str(switched_section(document, name));
            }
            _ => {
                let text = section(name);
                if lead_on && replaces_in(lead, name) {
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
    Some(assemble_steps(document, steps, true, false))
}

/// A preset's text with its dispatch split into "outside T3 Code" and the T3 sections in `suffix`.
pub(super) fn preset_text_for_t3(document: &Value, mode: &str, suffix: &str) -> Option<String> {
    let steps = catalog()["order"].get(mode)?.as_array()?;
    let mut text = assemble_steps(document, steps, true, true);
    text.push_str(suffix);
    Some(text)
}

/// The T3 sections a file gets in `mode` when it is read by a selected CLI that AgentNave
/// recognises inside T3 (`recognized`) and/or by selected ones it does not (`others`); None for the
/// modes without a T3 mode.
pub(super) fn t3_additions(mode: &str, recognized: bool, others: &[&str]) -> Option<String> {
    let base = t3_suffix(mode)?;
    let mut out = if recognized { base } else { String::new() };
    out.push_str(&t3_others(others));
    (!out.is_empty()).then_some(out)
}

/// The template of the section for CLIs AgentNave cannot recognise inside T3, `{names}` unfilled.
fn t3_others_template() -> String {
    catalog()["t3_other"]
        .as_array()
        .expect("t3_other array in orchestration_rules.json")
        .iter()
        .map(|step| section(step.as_str().expect("t3_other step string")))
        .collect()
}

fn t3_others(ids: &[&str]) -> String {
    if ids.is_empty() {
        return String::new();
    }
    let md = &catalog()["meeting_defaults"];
    let names: Vec<&str> = ids
        .iter()
        .filter_map(|id| md["provider_names"][*id].as_str())
        .collect();
    t3_others_template().replace("{names}", &names.join(str_at(md, "separator")))
}

/// The block of a boss that is on in T3 Code but off outside it: the shared rules limited to T3
/// Code, then the T3 sections.
pub(super) fn t3_only_text(document: &Value, recognized: bool, others: &[&str]) -> String {
    let only = &catalog()["t3_only"];
    let steps_of = |key: &str| -> Vec<Value> {
        only[key]
            .as_array()
            .unwrap_or_else(|| panic!("t3_only.{key} in orchestration_rules.json"))
            .clone()
    };
    let mut text = assemble_steps(document, &steps_of("base"), true, false);
    if recognized {
        for step in steps_of("recognized") {
            text.push_str(section(step.as_str().expect("t3_only step string")));
        }
    }
    text.push_str(&t3_others(others));
    text
}

pub(super) fn t3_suffix(mode: &str) -> Option<String> {
    let steps = catalog()["t3_order"].get(mode)?.as_array()?;
    let mut out = String::new();
    for step in steps {
        let name = step
            .as_str()
            .expect("t3_order step string in orchestration_rules.json");
        out.push_str(section(name));
    }
    (!out.is_empty()).then_some(out)
}

fn split_role_base(document: &Value) -> String {
    let steps = catalog()["order"]["split"]
        .as_array()
        .expect("split order in orchestration_rules.json");
    let end = steps
        .iter()
        .position(|step| step.as_str() == Some("enabled_roles"))
        .unwrap_or(steps.len());
    assemble_steps(document, &steps[..end], false, false)
}

fn dispatch_parts(tail: &str) -> Value {
    json!({
        "base": section("shared"),
        "parallel_writes_asked": section("parallel_writes_when_asked"),
        "parallel_writes_own": section("parallel_writes_by_boss"),
        "checks": section("shared_checks"),
        "meetings_asked": section("meetings_when_asked"),
        "meetings_own": section("meetings_on_tradeoffs"),
        "tail": section(tail),
    })
}

pub(super) fn presets_payload(document: &Value) -> Value {
    let rules = catalog();
    let md = &rules["meeting_defaults"];
    let lead = &rules["switches"]["lead_only"];
    let default_count = md["default_count"]
        .as_u64()
        .expect("meeting default_count in orchestration_rules.json");
    let t3: serde_json::Map<String, Value> = rules["t3_order"]
        .as_object()
        .expect("t3_order object in orchestration_rules.json")
        .keys()
        .filter_map(|mode| Some((mode.clone(), json!(t3_suffix(mode)?))))
        .collect();
    json!({"meetings": super::text_of(document, "meetings"),
        "split": super::text_of(document, "split"),
        "decision": super::text_of(document, "decision"),
        "meeting": meeting_line(document),
        "meetings_parts": {"head": section("meetings_only_head"), "tail": section("meetings_only_tail"), "meetings_asked": section("meetings_when_asked"), "meetings_own": section("meetings_on_tradeoffs")},
        "meeting_parts": {"default_count": default_count, "separator": str_at(md, "separator"),
            "seat_provider": str_at(md, "seat_provider"),
            "all": str_at(md, "all_named"), "some": str_at(md, "some_named"), "none": str_at(md, "none_named")},
        "image_generation": section("image_generation"),
        "role_sections": super::roles::sections(false),
        "role_sections_medium": super::roles::sections(true),
        "role_base": {"split": split_role_base(document)},
        "decision_parts": dispatch_parts("dispatch_decision"),
        "split_parts": dispatch_parts("dispatch_split"),
        "t3": Value::Object(t3),
        "t3_other": {"template": t3_others_template(), "names": md["provider_names"],
            "separator": str_at(md, "separator")},
        "non_t3_heading": section("t3_non_t3_heading"),
        "lead": {"asked": str_at(lead, "when_false"), "always": str_at(lead, "when_true")}})
}
