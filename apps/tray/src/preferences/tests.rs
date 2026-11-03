use super::*;

#[test]
fn updates_merge_fields_and_keep_unknown_ones() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("config");
    assert_eq!(state(&folder)["preferences"], json!({}));
    assert_eq!(language(&folder), "system");
    fs::create_dir_all(&folder).unwrap();
    fs::write(
        folder.join(FILE),
        br#"{"general":{"future":1},"later":{"x":true}}"#,
    )
    .unwrap();
    update(&folder, &json!({"general": {"language": "en"}})).unwrap();
    assert_eq!(
        read(&folder).unwrap(),
        *json!({"general": {"future": 1, "language": "en"}, "later": {"x": true}})
            .as_object()
            .unwrap()
    );
    assert_eq!(language(&folder), "en");
    for bad in [
        json!({"general": {"language": "fr"}}),
        json!({"general": {"typo": 1}}),
        json!({"nowhere": {"language": "en"}}),
        json!({"general": "en"}),
        json!([]),
    ] {
        assert!(update(&folder, &bad).is_err(), "{bad}");
    }
    assert_eq!(language(&folder), "en");
}

#[test]
fn a_broken_file_reads_empty_and_is_replaced_by_the_next_change() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join(FILE), b"{oops").unwrap();
    assert!(state(dir.path())["error"].is_string());
    assert_eq!(language(dir.path()), "system");
    update(dir.path(), &json!({"general": {"language": "zh"}})).unwrap();
    assert_eq!(state(dir.path())["error"], Value::Null);
    assert_eq!(language(dir.path()), "zh");
}

#[test]
fn pages_open_with_preferences_before_first_paint() {
    let html = b"<!doctype html>\n<html lang=\"zh-CN\">\n<head>";
    let general = general::effective(
        &json!({"general": {
            "language": "en", "theme": "dark", "motion": "reduce"
        }}),
        false,
    );
    let page = String::from_utf8(localize(html, &general)).unwrap();
    assert!(
        page.contains(r#"<html lang="en" data-lang="en" data-theme="dark" data-motion="reduce">"#)
    );
    assert_eq!(localize(b"<html>", &general), b"<html>");
}

#[test]
fn general_validates_all_contract_fields_without_accepting_coercions() {
    let dir = tempfile::tempdir().unwrap();
    for (field, values) in [
        ("language", vec![json!("system"), json!("zh"), json!("en")]),
        (
            "theme",
            vec![json!("system"), json!("light"), json!("dark")],
        ),
        (
            "motion",
            vec![json!("system"), json!("reduce"), json!("full")],
        ),
        ("scale", vec![json!(90), json!(100), json!(115)]),
        ("tray_click", vec![json!("panel"), json!("office")]),
        ("autostart", vec![json!(true), json!(false)]),
        ("open_on_start", vec![json!(true), json!(false)]),
        ("tray_count", vec![json!(true), json!(false)]),
        ("keep_awake", vec![json!(true), json!(false)]),
    ] {
        for value in values {
            assert!(general::validate(field, &value).is_ok(), "{field} {value}");
        }
        for value in [json!(null), json!({}), json!([]), json!("true"), json!(0)] {
            assert!(general::validate(field, &value).is_err(), "{field} {value}");
        }
    }
    for value in [json!(100.0), json!("100"), json!(101), json!(-90)] {
        assert!(update(dir.path(), &json!({"general": {"scale": value}})).is_err());
    }
    assert!(general::validate("theme", &json!("auto")).is_err());
    assert!(general::validate("motion", &json!("none")).is_err());
}

#[test]
fn effective_fills_defaults_and_uses_the_os_autostart_value() {
    let defaults = general::effective(&json!({}), true);
    assert_eq!(
        defaults,
        json!({
            "language": "system", "theme": "system", "motion": "system", "scale": 100,
            "autostart": true, "open_on_start": true, "tray_click": "panel",
            "tray_count": true, "keep_awake": false,
        })
    );
    let effective = general::effective(
        &json!({"general": {
            "autostart": true, "scale": 115, "theme": "light", "motion": "full",
            "open_on_start": false, "tray_click": "office", "tray_count": false, "keep_awake": true,
            "future": 1
        }}),
        false,
    );
    assert_eq!(effective["autostart"], false);
    assert_eq!(effective["scale"], 115);
    assert_eq!(effective["motion"], "full");
    assert_eq!(effective["tray_click"], "office");
    assert_eq!(effective["tray_count"], false);
    assert!(effective.get("future").is_none());
    assert_eq!(
        general::effective(
            &json!({"general": {"theme": "<script>", "scale": 1}}),
            false
        )["theme"],
        "system"
    );
}

#[test]
fn preview_refuses_autostart_even_when_disabling_and_before_writing() {
    for enabled in [false, true] {
        let changes = json!({"general": {"autostart": enabled, "theme": "light"}});
        assert!(general::reject_autostart(true, &changes).is_err());
        assert!(general::reject_autostart(false, &changes).is_ok());
        if crate::PREVIEW {
            let dir = tempfile::tempdir().unwrap();
            assert!(update(dir.path(), &changes).is_err());
            assert!(!dir.path().join(FILE).exists());
        }
    }
    assert!(general::reject_autostart(true, &json!({"general": {"theme": "dark"}})).is_ok());
}

#[test]
fn native_menu_and_html_resolve_system_language_consistently() {
    assert_eq!(general::resolved_language("system", "en-US"), "en");
    assert_eq!(general::resolved_language("system", "zh-Hant-TW"), "zh");
    assert_eq!(general::resolved_language("en", "zh-CN"), "en");
    assert_eq!(general::resolved_language("zh", "en-US"), "zh");
}

#[test]
fn get_response_backfills_autostart_and_exposes_effective_values_without_writing() {
    let dir = tempfile::tempdir().unwrap();
    update(
        dir.path(),
        &json!({"general": {"theme": "light", "scale": 115}}),
    )
    .unwrap();
    let before = fs::read(dir.path().join(FILE)).unwrap();
    let actual = runtime::response(state(dir.path()), true, 115, false);
    assert_eq!(actual["preferences"]["general"]["autostart"], true);
    assert_eq!(actual["effective"]["general"]["autostart"], true);
    assert_eq!(actual["effective"]["general"]["scale"], 115);
    assert_eq!(actual["effective"]["general"]["theme"], "light");
    assert_eq!(actual["autostart_available"], true);
    assert_eq!(fs::read(dir.path().join(FILE)).unwrap(), before);
    let preview = runtime::response(state(dir.path()), false, 100, true);
    assert_eq!(preview["autostart_available"], false);
    assert_eq!(preview["preferences"]["general"]["autostart"], false);
}
