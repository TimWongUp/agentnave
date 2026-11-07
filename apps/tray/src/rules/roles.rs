//! Generated role lines are replaceable; arbitrary custom text stays the user's.
use serde_json::{Value, json};

const LEGACY_DISPATCH: &str = r#"- Explorer（只读：探索、规划、评审）与 Worker（可写：实现）各有简单、困难两档，按任务难度二选一；涉及界面（页面、样式、交互、看截图或设计稿）的实现改用 Worker 的界面两档。取 `list_members` 的 `orchestration`：`native` 工位交给宿主自带子代理并尽量用该模型，它不经过服务，prompt 要写明非交互，Explorer 还要写明只读；其余派发时 `seat` 用 `explorer-easy`、`explorer-hard`、`worker-easy`、`worker-hard`、`worker-frontend-easy` 或 `worker-frontend-hard`，省略 `provider`、`access` 和模型，服务按 App 工位补齐；`role` 写具体职责，新任务必传 `cwd`。工位不可用时用 `list_members` 的 `replacement`，连同原来的 `seat` 或 `role` 一起传；没有工位时按任务组员选，都不可用时自己做。"#;
const LEGACY_ASSIGNMENT: &str = "- 探索、规划派 Explorer，实现派 Worker。";

pub(super) fn sections() -> Value {
    let mut sections = serde_json::Map::new();
    if let Some(catalog) = crate::orchestration_roles::catalog().as_array() {
        for role in catalog {
            let group = role["group"].as_str().expect("role group");
            let prefix = role["seat_prefix"].as_str().expect("seat prefix");
            let name = role["name_zh"].as_str().expect("role name");
            let access = if role["access"] == "read" { "只读" } else { "可写" };
            sections.insert(group.into(), json!(format!(
                "\n- {name}（{access}）：简单用 `{prefix}-easy`，困难用 `{prefix}-hard`。"
            )));
        }
    }
    Value::Object(sections)
}

pub(super) fn apply(document: &Value, mut text: String) -> String {
    // These exact sentences belonged to our older generated preset, including copies in Custom.
    text = text.replace(LEGACY_DISPATCH, "").replace(LEGACY_ASSIGNMENT, "");
    let sections = sections();
    let sections = sections.as_object().expect("role sections");
    for section in sections.values().filter_map(Value::as_str) {
        text = text.replace(section, "");
    }
    // Use catalogue order rather than JSON map sorting for a stable, readable directory.
    if let Some(catalog) = crate::orchestration_roles::catalog().as_array() {
        for role in catalog {
            let group = role["group"].as_str().expect("role group");
            if crate::orchestration_roles::enabled(document, group) {
                text.push_str(sections[group].as_str().expect("role section"));
            }
        }
    }
    text
}
