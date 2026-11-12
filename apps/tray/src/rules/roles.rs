//! Generated role lines are replaceable; arbitrary custom text stays the user's.
use serde_json::{Value, json};

const LEGACY_DISPATCH: &str = r#"- Explorer（只读：探索、规划、评审）与 Worker（可写：实现）各有简单、困难两档，按任务难度二选一；涉及界面（页面、样式、交互、看截图或设计稿）的实现改用 Worker 的界面两档。取 `list_members` 的 `orchestration`：`native` 工位交给宿主自带子代理并尽量用该模型，它不经过服务，prompt 要写明非交互，Explorer 还要写明只读；其余派发时 `seat` 用 `explorer-easy`、`explorer-hard`、`worker-easy`、`worker-hard`、`worker-frontend-easy` 或 `worker-frontend-hard`，省略 `provider`、`access` 和模型，服务按 App 工位补齐；`role` 写具体职责，新任务必传 `cwd`。工位不可用时用 `list_members` 的 `replacement`，连同原来的 `seat` 或 `role` 一起传；没有工位时按任务组员选，都不可用时自己做。"#;
const LEGACY_SMALL_TASKS: &str = "用户点名 CLI 时也通过 AgentNave 派发，不在终端直接运行它。小事自己做，但落在下列专项范围内的任务交给专项，即使自己的工具也能做。";
const LEGACY_PARALLEL: &str = "- 并行写入和开会由 boss 判断：计划确认后，互不依赖的写入步骤并行派给多个可写工位，boss 合入当前分支，最后统一验收；有实质取舍或分歧时开 AgentNave 会议室出决定，会上不改代码。";
const LEGACY_ASSIGNMENT: &str = "- 探索、规划派 Explorer，实现派 Worker。";
const LEGACY_REVIEW: &str = "- 本偏好不决定何时评审，用户要求或其规则要求时才评审。评审时有 agentnave-review Skill 就按它定级、选工位，不另派 Explorer；没有时派 Explorer，不交给写这段代码的 Worker，尽量换模型。";

/// What replaced the old one-line "small tasks" rule: from the named-CLI sentence through the
/// orchestration-only line.
fn small_tasks() -> String {
    let shared = super::catalog::section("shared");
    let start = shared
        .find("用户点名 CLI 时也通过 AgentNave 派发")
        .expect("named CLI sentence");
    let rest = &shared[start..];
    let end = rest.find("自己只拆分").expect("orchestration-only line");
    let end = end + rest[end..].find('\n').unwrap_or(rest.len() - end);
    rest[..end].to_owned()
}

fn directory_line(role: &Value) -> String {
    let template = &super::catalog::catalog()["enabled_roles"];
    let prefix = role["seat_prefix"].as_str().expect("seat prefix");
    let name = role["name_zh"].as_str().expect("role name");
    let scope = role["scope_zh"].as_str().expect("role scope");
    let access_key = role["access"].as_str().expect("role access");
    let access = template["access_zh"][access_key]
        .as_str()
        .unwrap_or_else(|| panic!("missing access label {access_key}"));
    template["line"]
        .as_str()
        .expect("enabled_roles line template")
        .replace("{name_zh}", name)
        .replace("{access_zh}", access)
        .replace("{scope_zh}", scope)
        .replace("{seat_prefix}", prefix)
}

pub(super) fn sections() -> Value {
    let mut sections = serde_json::Map::new();
    if let Some(catalog) = crate::orchestration_roles::catalog().as_array() {
        for role in catalog {
            let group = role["group"].as_str().expect("role group");
            sections.insert(group.into(), json!(directory_line(role)));
        }
    }
    Value::Object(sections)
}

pub(super) fn enabled_lines(document: &Value) -> String {
    let sections = sections();
    let sections = sections.as_object().expect("role sections");
    // Use catalogue order rather than JSON map sorting for a stable, readable directory.
    let enabled = crate::orchestration_roles::enabled_groups(document).unwrap_or_default();
    let mut out = String::new();
    if let Some(catalog) = crate::orchestration_roles::catalog().as_array() {
        for role in catalog {
            let group = role["group"].as_str().expect("role group");
            if enabled.contains(&group) {
                out.push_str(sections[group].as_str().expect("role section"));
            }
        }
    }
    out
}

/// The exact sentences of our older generated presets, including copies in Custom, replaced by
/// their current wording; anything else the user wrote stays as it is.
pub(super) fn migrate_legacy(text: &str) -> String {
    let shared = super::catalog::section("shared");
    let dispatch = shared
        .lines()
        .find(|line| line.contains("同难度专项优先"))
        .expect("shared dispatch instruction");
    let review = shared
        .lines()
        .find(|line| line.starts_with("- 本偏好不决定何时评审"))
        .expect("shared review instruction");
    text.replace(LEGACY_DISPATCH, dispatch)
        .replace(LEGACY_ASSIGNMENT, "")
        .replace(LEGACY_SMALL_TASKS, &small_tasks())
        .replace(
            LEGACY_PARALLEL,
            &format!(
                "{}{}",
                super::catalog::section("parallel_writes").trim_start(),
                super::catalog::section("meetings_on_tradeoffs")
            ),
        )
        .replace("互不依赖的写入步骤并行派给多个 Worker", "互不依赖的写入步骤并行派给多个可写工位")
        .replace(
            "互不依赖的写入步骤并行派给多个可写工位",
            "互不依赖的写入步骤各用独立 worktree 并行派给多个可写工位",
        )
        .replace(
            "各用独立 worktree，做不到就串行。",
            "各用独立 worktree，做不到就串行；项目规定不建 worktree（如要求直接在 main 修改）时也算做不到。",
        )
        .replace("同一组文件同一时间只有一个 Worker 在写", "同一组文件同一时间只有一个执行者在写")
        .replace("各用独立 worktree，做不到就串行；", "各用独立的隔离目录，做不到就串行；")
        .replace(
            "互不依赖的写入步骤各用独立 worktree 并行派给多个可写工位",
            "互不依赖的写入步骤各用独立的隔离目录并行派给多个可写工位",
        )
        .replace("每次最多等十分钟", "每次最多等 270 秒")
        .replace(LEGACY_REVIEW, review)
}

pub(super) fn apply(document: &Value, text: String) -> String {
    let mut text = migrate_legacy(&text);
    let sections = sections();
    let sections = sections.as_object().expect("role sections");
    let generated: Vec<String> = sections
        .values()
        .filter_map(Value::as_str)
        .map(|section| section.trim_start_matches('\n').to_owned())
        .collect();
    text = text
        .split_inclusive('\n')
        .filter(|line| {
            !generated
                .iter()
                .any(|known| known == line.trim_end_matches(['\r', '\n']))
        })
        .collect();
    text.truncate(text.trim_end().len());
    text.push_str(&enabled_lines(document));
    text
}

#[cfg(test)]
#[test]
fn custom_migration_keeps_dispatch_and_cleans_generated_first_lines() {
    let document = json!({"enabled_roles": []});
    let legacy = format!(
        "{LEGACY_DISPATCH}\n{LEGACY_ASSIGNMENT}\n- 计划确认后，互不依赖的写入步骤并行派给多个 Worker，在 Git 仓库里各用独立 worktree；无法隔离时串行。boss 合入当前分支，最后统一验收。\n- 选择本档即授权主动派发；{LEGACY_SMALL_TASKS}\n用户正文"
    );
    let migrated = apply(&document, legacy);
    assert!(migrated.contains("只负责编排") && !migrated.contains("小事自己做，但"));
    assert!(migrated.contains("同难度专项优先"));
    assert!(migrated.contains("兜底不跨难度"));
    assert!(!migrated.contains("Worker"));
    assert!(migrated.contains("用户正文"));
    let section = sections()["worker_writing"]
        .as_str()
        .unwrap()
        .trim()
        .to_owned();
    let first = apply(&document, format!("{section}\n用户正文"));
    assert_eq!(first, "用户正文");
    let last = apply(&document, format!("用户正文\n{section}"));
    assert_eq!(last, "用户正文");
}
