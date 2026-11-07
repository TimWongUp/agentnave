//! Generated role lines are replaceable; arbitrary custom text stays the user's.
use serde_json::{Value, json};

const LEGACY_DISPATCH: &str = r#"- Explorer（只读：探索、规划、评审）与 Worker（可写：实现）各有简单、困难两档，按任务难度二选一；涉及界面（页面、样式、交互、看截图或设计稿）的实现改用 Worker 的界面两档。取 `list_members` 的 `orchestration`：`native` 工位交给宿主自带子代理并尽量用该模型，它不经过服务，prompt 要写明非交互，Explorer 还要写明只读；其余派发时 `seat` 用 `explorer-easy`、`explorer-hard`、`worker-easy`、`worker-hard`、`worker-frontend-easy` 或 `worker-frontend-hard`，省略 `provider`、`access` 和模型，服务按 App 工位补齐；`role` 写具体职责，新任务必传 `cwd`。工位不可用时用 `list_members` 的 `replacement`，连同原来的 `seat` 或 `role` 一起传；没有工位时按任务组员选，都不可用时自己做。"#;
const LEGACY_ASSIGNMENT: &str = "- 探索、规划派 Explorer，实现派 Worker。";
const LEGACY_REVIEW: &str = "- 本偏好不决定何时评审，用户要求或其规则要求时才评审。评审时有 agentnave-review Skill 就按它定级、选工位，不另派 Explorer；没有时派 Explorer，不交给写这段代码的 Worker，尽量换模型。";

pub(super) fn sections() -> Value {
    let mut sections = serde_json::Map::new();
    if let Some(catalog) = crate::orchestration_roles::catalog().as_array() {
        for role in catalog {
            let group = role["group"].as_str().expect("role group");
            let prefix = role["seat_prefix"].as_str().expect("seat prefix");
            let name = role["name_zh"].as_str().expect("role name");
            let access = if role["access"] == "read" {
                "只读"
            } else {
                "可写"
            };
            sections.insert(
                group.into(),
                json!(format!(
                    "\n- {name}（{access}）：简单用 `{prefix}-easy`，困难用 `{prefix}-hard`。"
                )),
            );
        }
    }
    Value::Object(sections)
}

pub(super) fn apply(document: &Value, mut text: String) -> String {
    // These exact sentences belonged to our older generated preset, including copies in Custom.
    let dispatch = super::SHARED
        .lines()
        .find(|line| line.contains("同难度专项优先"))
        .expect("shared dispatch instruction");
    text = text
        .replace(LEGACY_DISPATCH, dispatch)
        .replace(LEGACY_ASSIGNMENT, "")
        .replace("互不依赖的写入步骤并行派给多个 Worker", "互不依赖的写入步骤并行派给多个可写工位")
        .replace("同一组文件同一时间只有一个 Worker 在写", "同一组文件同一时间只有一个执行者在写")
        .replace(LEGACY_REVIEW, "- 本偏好不决定何时评审，用户要求或其规则要求时才评审。评审时有 agentnave-review Skill 就按它定级、选工位；没有时选择当前可见的只读工位，不可用时由 boss 自行评审。不交给实现者，尽量换模型。");
    let sections = sections();
    let sections = sections.as_object().expect("role sections");
    text = text
        .split_inclusive('\n')
        .filter(|line| {
            !sections.values().filter_map(Value::as_str).any(|section| {
                section.trim_start_matches('\n') == line.trim_end_matches(['\r', '\n'])
            })
        })
        .collect();
    // Use catalogue order rather than JSON map sorting for a stable, readable directory.
    let enabled = crate::orchestration_roles::enabled_groups(document).unwrap_or_default();
    if let Some(catalog) = crate::orchestration_roles::catalog().as_array() {
        for role in catalog {
            let group = role["group"].as_str().expect("role group");
            if enabled.contains(&group) {
                text.push_str(sections[group].as_str().expect("role section"));
            }
        }
    }
    text
}

#[cfg(test)]
#[test]
fn custom_migration_keeps_dispatch_and_cleans_generated_first_lines() {
    let document = json!({"enabled_roles": []});
    let legacy = format!(
        "{LEGACY_DISPATCH}\n{LEGACY_ASSIGNMENT}\n- 计划确认后，互不依赖的写入步骤并行派给多个 Worker，在 Git 仓库里各用独立 worktree；无法隔离时串行。boss 合入当前分支，最后统一验收。\n用户正文"
    );
    let migrated = apply(&document, legacy);
    assert!(migrated.contains("同难度专项优先"));
    assert!(migrated.contains("`cwd`"));
    assert!(!migrated.contains("Worker"));
    assert!(migrated.contains("用户正文"));
    let section = sections()["worker_writing"]
        .as_str()
        .unwrap()
        .trim()
        .to_owned();
    let first = apply(&document, format!("{section}\n用户正文"));
    assert_eq!(first, "用户正文");
}
