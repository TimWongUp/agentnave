use super::*;
use crate::terminal::DerivedLaunchConfig;
use crate::terminal::lifecycle::WakePlan;

#[test]
fn a_wake_keeps_the_launch_but_starts_in_the_mode_claude_fell_back_to() {
    let launched = RunConfig {
        provider: "claude".to_owned(),
        account: None,
        account_dir: None,
        cwd: "/tmp".to_owned(),
        isolation: None,
        model: Some("claude-sonnet".to_owned()),
        effort: Some("high".to_owned()),
        permissions: json!({"permission_mode": "auto", "permission_fallback": "bypassPermissions"}),
    };
    let mut plan = WakePlan {
        provider: "claude".to_owned(),
        cwd: PathBuf::from("/tmp"),
        native: "aaaaaaaa-1111-4111-8111-aaaaaaaaaaaa".to_owned(),
        account: None,
        config: Some(DerivedLaunchConfig::from_run_config(&launched)),
        fallback_mode: None,
        isolation_id: None,
    };
    let cfg = wake::wake_config(&plan).unwrap();
    assert_eq!(cfg.permissions, json!({"permission_mode": "auto"}));
    plan.fallback_mode = Some("bypassPermissions".to_owned());
    let cfg = wake::wake_config(&plan).unwrap();
    assert_eq!(
        cfg.permissions,
        json!({"permission_mode": "bypassPermissions"})
    );
    let hook_dir = claude_hook::prepare_claude_hook(false).unwrap();
    let args = build_claude_args(&cfg, Some(&plan.native), None, Some(hook_dir.path()));
    assert!(
        args.windows(2)
            .any(|w| w == ["--resume", plan.native.as_str()])
    );
    assert!(args.windows(2).any(|w| w == ["--model", "claude-sonnet"]));
    let settings = args.iter().position(|a| a == "--settings").unwrap();
    let settings: Value = serde_json::from_str(&args[settings + 1]).unwrap();
    assert!(settings["hooks"]["Stop"].is_array());
    assert!(settings["hooks"].get("UserPromptSubmit").is_none());
    plan.config = None;
    assert!(wake::wake_config(&plan).is_err());
}
