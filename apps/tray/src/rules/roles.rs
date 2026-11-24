//! Generated role lines are replaceable; arbitrary custom text stays the user's.
use serde_json::{Value, json};

const LEGACY_DISPATCH: &str = r#"- Explorer（只读：探索、规划、评审）与 Worker（可写：实现）各有简单、困难两档，按任务难度二选一；涉及界面（页面、样式、交互、看截图或设计稿）的实现改用 Worker 的界面两档。取 `list_members` 的 `orchestration`：`native` 工位交给宿主自带子代理并尽量用该模型，它不经过服务，prompt 要写明非交互，Explorer 还要写明只读；其余派发时 `seat` 用 `explorer-easy`、`explorer-hard`、`worker-easy`、`worker-hard`、`worker-frontend-easy` 或 `worker-frontend-hard`，省略 `provider`、`access` 和模型，服务按 App 工位补齐；`role` 写具体职责，新任务必传 `cwd`。工位不可用时用 `list_members` 的 `replacement`，连同原来的 `seat` 或 `role` 一起传；没有工位时按任务组员选，都不可用时自己做。"#;
const LEGACY_SMALL_TASKS: &str = "用户点名 CLI 时也通过 AgentNave 派发，不在终端直接运行它。小事自己做，但落在下列专项范围内的任务交给专项，即使自己的工具也能做。";
const LEGACY_PARALLEL: &str = "- 并行写入和开会由 boss 判断：计划确认后，互不依赖的写入步骤并行派给多个可写工位，boss 合入当前分支，最后统一验收；有实质取舍或分歧时开 AgentNave 会议室出决定，会上不改代码。";
const LEGACY_ASSIGNMENT: &str = "- 探索、规划派 Explorer，实现派 Worker。";
const LEGACY_REVIEW: &str = "- 本偏好不决定何时评审，用户要求或其规则要求时才评审。评审时有 agentnave-review Skill 就按它定级、选工位，不另派 Explorer；没有时派 Explorer，不交给写这段代码的 Worker，尽量换模型。";
const LEGACY_SPECIALIST_SMALL_TASKS: &str = "boss 只自己做小事：读一两个文件、改一处、几分钟能完成，且不在下列专项范围内；多文件实现、需要调查定位的问题、成篇文案和专项范围内的任务都派出，即使自己的工具也能做。";
const LEGACY_WRITING_SMALL_TASKS: &str = "boss 只自己做小事：读一两个文件、改一处、几分钟能完成；多文件实现、需要调查定位的问题和成篇文案都派出，即使自己的工具也能做。";
const CURRENT_SMALL_TASKS: &str = "boss 只自己做小事：读一两个文件、改一处、几分钟能完成；多文件实现、需要调查定位的问题和成篇文案都派出，即使自己的工具也能做；写作所需的上下文已全在 boss 手里、且未要求 boss 只负责编排时除外，例如刚收敛的讨论或会议结论要写成 ADR、架构文档、术语表或定稿说明，简报几乎就是正文，boss 自己写；需要另外调研或上下文不在 boss 手里的成篇写作仍派出。";
const LEGACY_WAIT_VERIFY: &str = "- 以回执 `next` 为准：未注明 App 会提示时，结束本轮前用 `wait` 收齐；注明时可直接结束，收到提示后用收件箱 `wait(wait_seconds=0)` 收取；`succeeded` 不等于完成，boss 核对产物后验收；高风险改动由 boss 针对具体的出错方式补做检查。";
const LEGACY_MEETINGS_WAIT: &str = "- 以回执 `next` 为准：未注明 App 会提示时，结束本轮前用 `wait` 收齐；注明时可直接结束，收到提示后用收件箱 `wait(wait_seconds=0)` 收取。";
const LEGACY_AUTH_LOAD: &str = "- 本偏好不新增推送、合并 PR、发布、发送的授权，也不追加权限参数；派发、开会、等待和验收前先加载 agentnave-manager Skill 并按它执行。";

/// What replaced the old one-line "small tasks" rule: from the named-CLI sentence through the
/// orchestration-only line.
fn small_tasks() -> String {
    let shared = super::catalog::section("shared");
    let start = shared
        .find("- 派发是默认做法")
        .expect("default dispatch line");
    let rest = &shared[start..];
    let end = rest.find("自己只拆分").expect("orchestration-only line");
    let end = end + rest[end..].find('\n').unwrap_or(rest.len() - end);
    format!(
        "用户点名 CLI 时也通过 AgentNave 派发，不在终端直接运行它。\n{}",
        &rest[..end]
    )
}

/// A group's directory line; with its optional medium seat bound, the line names that seat too.
fn directory_line(role: &Value, medium: bool) -> String {
    let template = &super::catalog::catalog()["enabled_roles"];
    let key = if medium { "line_medium" } else { "line" };
    let prefix = role["seat_prefix"].as_str().expect("seat prefix");
    let name = role["name_zh"].as_str().expect("role name");
    let scope = role["scope_zh"].as_str().expect("role scope");
    let access_key = role["access"].as_str().expect("role access");
    let access = template["access_zh"][access_key]
        .as_str()
        .unwrap_or_else(|| panic!("missing access label {access_key}"));
    template[key]
        .as_str()
        .unwrap_or_else(|| panic!("missing enabled_roles {key} template"))
        .replace("{name_zh}", name)
        .replace("{access_zh}", access)
        .replace("{scope_zh}", scope)
        .replace("{seat_prefix}", prefix)
}

/// Each group's line, without or with its medium seat.
pub(super) fn sections(medium: bool) -> Value {
    let mut sections = serde_json::Map::new();
    if let Some(catalog) = crate::orchestration_roles::catalog().as_array() {
        for role in catalog {
            let group = role["group"].as_str().expect("role group");
            sections.insert(group.into(), json!(directory_line(role, medium)));
        }
    }
    Value::Object(sections)
}

pub(super) fn enabled_lines(document: &Value) -> String {
    // Use catalogue order rather than JSON map sorting for a stable, readable directory.
    let enabled = crate::orchestration_roles::enabled_groups(document).unwrap_or_default();
    let mut out = String::new();
    if let Some(catalog) = crate::orchestration_roles::catalog().as_array() {
        for role in catalog {
            let group = role["group"].as_str().expect("role group");
            if enabled.contains(&group) {
                out.push_str(&directory_line(role, document[group]["medium"].is_object()));
            }
        }
    }
    out
}

/// The exact sentences of our older generated presets, including copies in Custom, replaced by
/// their current wording; anything else the user wrote stays as it is.
pub(super) fn migrate_legacy(text: &str) -> String {
    let shared = super::catalog::section("shared");
    let shared_checks = super::catalog::section("shared_checks");
    let dispatch = super::catalog::section("dispatch_split")
        .lines()
        .find(|line| line.contains("同难度专项优先"))
        .expect("split dispatch instruction");
    let review = shared_checks
        .lines()
        .find(|line| line.contains("不可用时由 boss 自行评审"))
        .expect("shared review instruction");
    let wait_verify = shared_checks
        .lines()
        .find(|line| line.starts_with("- `succeeded` 不等于完成"))
        .expect("shared verify instruction");
    let auth_load = shared
        .lines()
        .find(|line| line.starts_with("- 本偏好不新增推送、合并 PR、发布、发送的授权"))
        .expect("shared auth/load instruction");
    let meetings_wait = super::catalog::section("meetings_only_tail").trim();
    text.replace(LEGACY_DISPATCH, dispatch)
        .replace(LEGACY_ASSIGNMENT, "")
        .replace(LEGACY_SMALL_TASKS, &small_tasks())
        .replace(LEGACY_SPECIALIST_SMALL_TASKS, CURRENT_SMALL_TASKS)
        .replace(LEGACY_WRITING_SMALL_TASKS, CURRENT_SMALL_TASKS)
        .replace(
            LEGACY_PARALLEL,
            &format!(
                "{}{}",
                super::catalog::section("parallel_writes_by_boss").trim_start(),
                super::catalog::section("meetings_on_tradeoffs")
            ),
        )
        .replace(
            "互不依赖的写入步骤并行派给多个 Worker",
            "互不依赖的写入步骤并行派给多个可写工位",
        )
        .replace(
            "各用独立 worktree，做不到就串行。",
            "各用独立 worktree，做不到就串行；项目规定不建 worktree（如要求直接在 main 修改）时也算做不到。",
        )
        .replace(
            "同一组文件同一时间只有一个 Worker 在写",
            "同一组文件同一时间只有一个执行者在写",
        )
        .replace(
            "各用独立 worktree，做不到就串行；",
            "各用独立的隔离目录，做不到就串行；",
        )
        .replace(
            "互不依赖的写入步骤各用独立 worktree 并行派给多个可写工位",
            "互不依赖的写入步骤各用独立的隔离目录并行派给多个可写工位",
        )
        .replace("每次最多等十分钟", "每次最多等 270 秒")
        .replace(LEGACY_REVIEW, review)
        .replace(LEGACY_WAIT_VERIFY, wait_verify)
        .replace(LEGACY_MEETINGS_WAIT, meetings_wait)
        .replace(LEGACY_AUTH_LOAD, auth_load)
}

pub(super) fn apply(document: &Value, text: String) -> String {
    let mut text = migrate_legacy(&text);
    let generated: Vec<String> = [sections(false), sections(true)]
        .iter()
        .filter_map(Value::as_object)
        .flat_map(|sections| sections.values())
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
    let section = sections(false)["worker_writing"]
        .as_str()
        .unwrap()
        .trim()
        .to_owned();
    let first = apply(&document, format!("{section}\n用户正文"));
    assert_eq!(first, "用户正文");
    let last = apply(&document, format!("用户正文\n{section}"));
    assert_eq!(last, "用户正文");
    let specialist_and_wait = apply(
        &document,
        format!("{LEGACY_SPECIALIST_SMALL_TASKS}\n{LEGACY_WAIT_VERIFY}\n{LEGACY_MEETINGS_WAIT}"),
    );
    assert!(specialist_and_wait.contains(CURRENT_SMALL_TASKS));
    assert!(!specialist_and_wait.contains("不在下列专项范围内"));
    assert!(super::catalog::section("shared").contains(CURRENT_SMALL_TASKS));
    assert_eq!(
        apply(&document, LEGACY_WRITING_SMALL_TASKS.to_owned()),
        CURRENT_SMALL_TASKS
    );
    assert!(!specialist_and_wait.contains("wait(wait_seconds=0)"));
    assert!(
        specialist_and_wait.contains("boss 核对结论与产物后验收；用隔离目录写的改动先逐项核对")
    );
    let migrated_auth = apply(
        &document,
        format!("{LEGACY_AUTH_LOAD}\n用户自定义：保留这行。"),
    );
    assert!(!migrated_auth.contains("派发、开会、等待和验收前先加载"));
    assert!(
        migrated_auth.contains("本偏好不新增推送、合并 PR、发布、发送的授权，也不追加权限参数。")
    );
    assert!(migrated_auth.contains("用户自定义：保留这行。"));
}
