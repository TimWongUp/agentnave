use super::*;

const KEY: &str = "mcp_servers";

fn parse(raw: &str) -> Result<Document, String> {
    Document::parse(raw, Format::Yaml, KEY)
}

/// Writes `entry`, verifies it, and checks a second parse renders the same bytes.
fn write(doc: &Document, entry: &Value) -> String {
    let written = doc.render(Some(entry)).unwrap();
    doc.verify(&written, Some(entry)).unwrap();
    let after = parse(&written).unwrap();
    assert_eq!(after.entry.as_ref(), Some(entry), "{written}");
    assert_eq!(after.render(Some(entry)).unwrap(), written);
    written
}

// Hermes' default `uwu` personality is a multi-line plain scalar whose continuation starts with
// `*`; other opaque bodies may hold an indented `---` or a tab inside text.
const OPAQUE: &str = "agent:\n  personalities:\n    uwu: hewwo! i'm your fwiendwy assistant uwu~\n      *nuzzles your code* OwO what's this?\n    doc: |\n      ---\n      a\tb &ref\n    alias: &kept value\n";

#[test]
fn opaque_bodies_with_yaml_indicators_stay_byte_identical() {
    let raw = format!("{OPAQUE}mcp_servers:\n  agentnave:\n    command: \"/old\"\n    env: {{}}\n");
    let doc = parse(&raw).unwrap();
    let mut entry = doc.entry.clone().unwrap();
    entry["env"]["AGENTNAVE_HOST"] = json!("hermes");
    let written = write(&doc, &entry);
    assert!(written.starts_with(OPAQUE), "{written}");
    let removed = doc.render(None).unwrap();
    doc.verify(&removed, None).unwrap();
    assert_eq!(removed, format!("{OPAQUE}mcp_servers:\n"));
    let fresh = parse(OPAQUE).unwrap();
    let written = write(&fresh, &entry);
    assert!(written.starts_with(OPAQUE), "{written}");
}

// ruamel.yaml folds a long flow map at 80 columns.
const FOLDED: &str = "mcp_servers:\n  other:\n    command: keep\n  agentnave:\n    command: \"/old\"\n    env: {\"AGENTNAVE_EXCLUDED_PROVIDERS\": \"\", \"AGENTNAVE_HOST\": \"hermes\", \"AGENTNAVE_HOST_APP\": \"hermes\",\n      \"CODEBUDDY_CONFIG_DIR\": \"/x/.workbuddy\", \"PATH\": \"/bin:/usr/bin\"} # managed\n\n    # note\n    enabled: true\n    timeout: 360\ntail: keep\n";

#[test]
fn folded_flow_env_is_read_and_rewritten_without_leftover_lines() {
    for eol in ["\n", "\r\n"] {
        let raw = FOLDED.replace('\n', eol);
        let doc = parse(&raw).unwrap();
        let entry = doc.entry.clone().unwrap();
        assert_eq!(entry["env"]["PATH"], "/bin:/usr/bin");
        assert_eq!(entry["env"]["AGENTNAVE_EXCLUDED_PROVIDERS"], "");
        assert_eq!(entry["timeout"], 360);
        assert_eq!(doc.render(Some(&entry)).unwrap(), raw);

        let mut changed = entry.clone();
        let env = changed["env"].as_object_mut().unwrap();
        env.insert("NEW".into(), json!("1"));
        env.insert("PATH".into(), json!("/bin"));
        env.remove("AGENTNAVE_HOST_APP");
        let written = write(&doc, &changed);
        let line = format!(
            "    env: {} # managed{eol}{eol}    # note{eol}",
            changed["env"]
        );
        assert!(written.contains(&line), "{written}");
        assert!(!written.contains("CODEBUDDY_CONFIG_DIR\": \"/x/.workbuddy\", "));
        assert!(written.ends_with(&format!("    timeout: 360{eol}tail: keep{eol}")));

        let removed = doc.render(None).unwrap();
        doc.verify(&removed, None).unwrap();
        assert_eq!(
            removed,
            format!("mcp_servers:{eol}  other:{eol}    command: keep{eol}tail: keep{eol}")
        );
    }
}

#[test]
fn unsupported_structures_on_the_traversed_path_are_still_rejected() {
    let entry = "mcp_servers:\n  agentnave:\n    command: x\n";
    let flow = "mcp_servers:\n  agentnave:\n    env: {\"A\": \"1\",\n";
    for raw in [
        format!("---\n{entry}"),
        format!("{entry}...\n"),
        format!("%YAML 1.2\n---\n{entry}"),
        format!("\tkey: x\n{entry}"),
        "mcp_servers:\n\tagentnave:\n    command: x\n".into(),
        format!("root: &a x\n{entry}"),
        format!("*a: x\n{entry}"),
        "mcp_servers: &servers\n  agentnave:\n    command: x\n".into(),
        "mcp_servers:\n  &a agentnave:\n    command: x\n".into(),
        "mcp_servers:\n  agentnave:\n    command: *cmd\n".into(),
        "mcp_servers:\n  agentnave:\n    args:\n    - &flag x\n".into(),
        "mcp_servers:\n  agentnave:\n    env:\n      KEY: &v x\n".into(),
        "mcp_servers:\n  agentnave:\n    env:\n      KEY: *v\n".into(),
        format!("{flow}      \"B\": \"2\"\n"),
        format!("{flow}      \"B\": \"2}}\n"),
        format!("{flow}      \"B\": *v}}\n"),
        format!("{flow}      &x \"B\": \"2\"}}\n"),
        "mcp_servers:\n  agentnave:\n    env: {A: 1,\n      B: 2}\n".into(),
        "mcp_servers:\n  agentnave:\n    env: {\"A\": \"1\"}\n      \"B\": \"2\"\n".into(),
        "mcp_servers:\n  agentnave:\n    env: [\"1\",\n      \"2\"]\n".into(),
    ] {
        assert!(parse(&raw).is_err(), "{raw}");
    }
}
