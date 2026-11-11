use super::*;

pub(super) fn newline(raw: &str) -> &'static str {
    if raw.contains("\r\n") { "\r\n" } else { "\n" }
}
pub(super) fn yaml_block(header: &str, body: &str, eol: &str) -> String {
    let separator = if !header.ends_with('\n') && !body.is_empty() {
        eol
    } else {
        ""
    };
    format!("{header}{separator}{body}")
}
pub(super) fn yaml_indent(raw: &str, fallback: usize) -> usize {
    raw.lines()
        .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
        .map(|l| l.len() - l.trim_start_matches(' ').len())
        .min()
        .unwrap_or(fallback)
}
pub(super) fn yaml_step(raw: &str) -> usize {
    raw.lines()
        .map(|l| l.len() - l.trim_start_matches(' ').len())
        .filter(|n| *n > 0)
        .min()
        .unwrap_or(2)
}
pub(super) fn yaml_patch(
    raw: &str,
    indent: usize,
    original: &Value,
    desired: &Value,
    step: usize,
    eol: &str,
) -> Result<String, String> {
    let mut changed = raw.to_owned();
    let fields = yaml_fields(raw, indent)?;
    for field in fields.iter().rev() {
        if original.get(&field.key) == desired.get(&field.key) {
            continue;
        }
        let replacement = if let Some(value) = desired.get(&field.key) {
            if field.value.is_empty() && value.is_object() {
                let body = &raw[field.body.clone()];
                let next = yaml_indent(body, indent + step);
                yaml_block(
                    &raw[field.full.start..field.body.start],
                    &yaml_patch(body, next, &original[&field.key], value, step, eol)?,
                    eol,
                )
            } else {
                // Preserve the key, spacing, trailing comment and any following blank lines.
                let header_end = raw[field.full.clone()]
                    .find('\n')
                    .map(|i| field.full.start + i)
                    .unwrap_or(field.full.end);
                let header = &raw[field.full.start..header_end];
                let colon = header.find(':').ok_or("YAML 映射无效")?;
                let tail = &header[colon + 1..];
                let start = field.full.start + colon + 1 + tail.len() - tail.trim_start().len();
                let mut line = raw[field.full.clone()].to_owned();
                line.replace_range(
                    start - field.full.start..start - field.full.start + field.value.len(),
                    &value.to_string(),
                );
                line
            }
        } else {
            String::new()
        };
        changed.replace_range(field.full.clone(), &replacement);
    }
    for (key, value) in desired.as_object().ok_or("YAML 映射无效")? {
        if original.get(key).is_none() {
            if !changed.is_empty() && !changed.ends_with('\n') {
                changed.push_str(eol);
            }
            changed.push_str(&yaml_line(key, value, indent, step, eol));
        }
    }
    Ok(changed)
}

pub(super) fn yaml_line(key: &str, value: &Value, indent: usize, step: usize, eol: &str) -> String {
    if let Some(object) = value.as_object().filter(|o| !o.is_empty()) {
        let mut out = format!("{}{key}:{eol}", " ".repeat(indent));
        for (key, value) in object {
            out.push_str(&yaml_line(key, value, indent + step, step, eol));
        }
        return out;
    }
    format!("{}{key}: {}{eol}", " ".repeat(indent), value)
}
pub(super) fn yaml_server(entry: &Value, indent: usize, step: usize, eol: &str) -> String {
    let mut out = format!("{}agentnave:{eol}", " ".repeat(indent));
    for (key, value) in entry.as_object().unwrap() {
        out.push_str(&yaml_line(key, value, indent + step, step, eol));
    }
    out
}

pub(super) struct YamlField {
    pub(super) key: String,
    pub(super) value: String,
    pub(super) indent: usize,
    pub(super) full: Range<usize>,
    pub(super) body: Range<usize>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BodyState {
    None,
    Empty,
    Sequence,
    Indented,
}

/// Only ordinary block mappings at the traversed levels. Unknown field bodies stay opaque.
/// Anchors, aliases, directives and duplicate mapping keys are rejected rather than rewritten.
pub(super) fn yaml_fields(raw: &str, indent: usize) -> Result<Vec<YamlField>, String> {
    let mut fields: Vec<YamlField> = Vec::new();
    let mut body = BodyState::None;
    let mut offset = 0;
    for line in raw.split_inclusive('\n') {
        let text = line.trim();
        if text.is_empty() || text.starts_with('#') {
            offset += line.len();
            continue;
        }
        if line.contains('\t')
            || text.starts_with(['&', '*', '%'])
            || text.starts_with("- &")
            || text.starts_with("- *")
            || text
                .split_once(':')
                .is_some_and(|(_, value)| value.trim_start().starts_with(['&', '*']))
            || text == "---"
            || text == "..."
        {
            return Err("不支持的 YAML 结构，需手动配置".into());
        }
        let leading = line.len() - line.trim_start_matches(' ').len();
        if leading < indent {
            return Err("YAML 缩进无效".into());
        }
        if leading == indent {
            if text == "-" || text.starts_with("- ") {
                if !matches!(body, BodyState::Empty | BodyState::Sequence) {
                    return Err("YAML 映射无效".into());
                }
                body = BodyState::Sequence;
                offset += line.len();
                continue;
            }
            let (key, value) = text.split_once(':').ok_or("YAML 映射无效")?;
            let key = key.trim_matches(['\'', '"']);
            if key.is_empty()
                || key.contains(['{', '}', '[', ']', '&', '*'])
                || fields.iter().any(|f| f.key == key)
            {
                return Err("YAML 键无效或重复".into());
            }
            if let Some(previous) = fields.last_mut() {
                previous.full.end = offset;
                previous.body.end = offset;
            }
            let value = yaml_value_text(value)?.to_owned();
            body = if value.is_empty() {
                BodyState::Empty
            } else {
                BodyState::None
            };
            fields.push(YamlField {
                key: key.into(),
                value,
                indent,
                full: offset..raw.len(),
                body: offset + line.len()..raw.len(),
            });
        } else if fields.is_empty() {
            return Err("YAML 缩进无效".into());
        } else if body == BodyState::Empty {
            body = BodyState::Indented;
        }
        offset += line.len();
    }
    Ok(fields)
}

pub(super) fn yaml_value_text(text: &str) -> Result<&str, String> {
    let mut single = false;
    let mut double = false;
    let mut escaped = false;
    let mut end = text.len();
    for (i, ch) in text.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if double => escaped = true,
            '\'' if !double => single = !single,
            '"' if !single => double = !double,
            '#' if !single
                && !double
                && (i == 0 || text.as_bytes()[i - 1].is_ascii_whitespace()) =>
            {
                end = i;
                break;
            }
            _ => {}
        }
    }
    if single || double {
        return Err("YAML 引号未闭合".into());
    }
    Ok(text[..end].trim())
}

pub(super) fn yaml_scalar(text: &str) -> Result<Value, String> {
    let text = yaml_value_text(text)?;
    if let Ok(value) = serde_json::from_str(text) {
        return Ok(value);
    }
    if text.starts_with('\'') && text.ends_with('\'') && text.len() >= 2 {
        return Ok(json!(text[1..text.len() - 1].replace("''", "'")));
    }
    if text.is_empty() || text.starts_with(['"', '[', '{', '&', '*', '|', '>']) {
        return Err("YAML 值需手动确认".into());
    }
    Ok(json!(text))
}

pub(super) fn yaml_entry(raw: &str, indent: usize) -> Result<Value, String> {
    let mut entry = json!({});
    for field in yaml_fields(raw, indent)? {
        let value = if field.key == "env" && field.value.is_empty() {
            let mut env = json!({});
            for item in yaml_fields(
                &raw[field.body.clone()],
                yaml_indent(&raw[field.body.clone()], indent + 2),
            )? {
                env[&item.key] = yaml_scalar(&item.value)?;
            }
            env
        } else if matches!(field.key.as_str(), "command" | "env") {
            yaml_scalar(&field.value)?
        } else if let Ok(v) = serde_json::from_str(&field.value) {
            v
        } else {
            // Opaque YAML is not reserialized during takeover/entry changes.
            json!({"$yaml": &raw[field.full.clone()]})
        };
        entry[&field.key] = value;
    }
    Ok(entry)
}

pub(super) const DSH_MCP_CLIENT: &str = "@deepseek-ai/dsh-mcp-client";

/// AgentNave's item in a dsh patch list and the `config` body of the row it inserts.
pub(super) struct DshItem {
    pub(super) full: Range<usize>,
    pub(super) config: Range<usize>,
    pub(super) indent: usize,
}

/// Column-0 lines that are not blank, comments included, with their byte offsets.
pub(super) fn dsh_lines(raw: &str) -> impl Iterator<Item = (usize, &str)> {
    raw.split_inclusive('\n')
        .scan(0, |offset, line| {
            let start = *offset;
            *offset += line.len();
            Some((start, line))
        })
        .filter(|(_, line)| !line.starts_with([' ', '\t']) && !line.trim().is_empty())
}

pub(super) fn dsh_empty(line: &str) -> bool {
    yaml_value_text(line).is_ok_and(|text| text == "[]")
}

/// Everything but AgentNave's item, ignoring an empty `[]` (its comment kept) and line endings.
pub(super) fn dsh_rest(rest: &Value) -> String {
    let mut lines: Vec<&str> = Vec::new();
    for line in rest.as_str().unwrap_or_default().lines() {
        if line.starts_with([' ', '\t']) || !dsh_empty(line) {
            lines.push(line.trim_end());
        } else if let Some(i) = line.find('#') {
            lines.push(line[i..].trim_end());
        }
    }
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}

pub(super) const DSH_MANUAL: &str = "DeepSeek Harness 补丁需为顶格书写的块式 YAML 列表（不支持 BOM、`---`、缩进或流式列表），需手动配置";

/// Top-level list items, each up to the next column-0 line. Only block lists are edited: any
/// other column-0 content than items, comments and an empty `[]` needs manual configuration.
pub(super) fn dsh_items(raw: &str) -> Result<Vec<Range<usize>>, String> {
    let mut items: Vec<Range<usize>> = Vec::new();
    // A list indented as a whole is valid YAML, but an appended column-0 item would add a root.
    let first = raw
        .lines()
        .find(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'));
    if first.is_some_and(|l| l.starts_with([' ', '\t'])) {
        return Err(DSH_MANUAL.into());
    }
    let lines: Vec<_> = dsh_lines(raw).collect();
    for (i, (offset, line)) in lines.iter().enumerate() {
        if line.starts_with("- ") || line.trim_end() == "-" {
            items.push(*offset..lines.get(i + 1).map_or(raw.len(), |next| next.0));
        } else if !line.starts_with('#') && !dsh_empty(line) {
            return Err(DSH_MANUAL.into());
        }
    }
    Ok(items)
}

/// The `- insert:` item whose single row is `id: agentnave`. Other items stay opaque, so their
/// `!!js` expressions are never parsed; a row sharing an item with others is left to the user.
pub(super) fn dsh_item(raw: &str) -> Result<Option<DshItem>, String> {
    let mut found = None;
    for item in dsh_items(raw)? {
        let text = &raw[item.clone()];
        let (head, body) = text.split_at(text.find('\n').map_or(text.len(), |i| i + 1));
        if yaml_value_text(head).ok() != Some("- insert:") {
            continue;
        }
        let manual = || {
            if body.contains("agentnave") {
                Err("DeepSeek Harness 的 agentnave 条目需手动配置".to_string())
            } else {
                Ok(())
            }
        };
        let indent = yaml_indent(body, 4);
        let mut rows = Vec::new();
        let mut offset = head.len();
        for line in body.split_inclusive('\n') {
            let content = !line.trim().is_empty() && !line.trim_start().starts_with('#');
            if content && line.len() - line.trim_start_matches(' ').len() == indent {
                if !line[indent..].starts_with("- ") {
                    manual()?;
                    rows.clear();
                    break;
                }
                rows.push(offset..text.len());
                if let Some(previous) = rows.iter_mut().rev().nth(1) {
                    previous.end = offset;
                }
            }
            offset += line.len();
        }
        for row in &rows {
            // Read the row as a mapping by blanking its dash; offsets stay the same.
            let masked = format!(
                "{}  {}",
                " ".repeat(indent),
                &text[row.start + indent + 2..row.end]
            );
            let Ok(fields) = yaml_fields(&masked, indent + 2) else {
                manual()?;
                continue;
            };
            let id = fields.iter().find(|f| f.key == "id");
            if !id.is_some_and(|f| yaml_scalar(&f.value).is_ok_and(|v| v == "agentnave")) {
                continue;
            }
            // dsh's plugin manager matches a row by id and name; another plugin's row is not ours.
            let name = fields.iter().find(|f| f.key == "name");
            let ours =
                name.is_some_and(|f| yaml_scalar(&f.value).is_ok_and(|v| v == DSH_MCP_CLIENT));
            let config = fields
                .iter()
                .find(|f| f.key == "config" && f.value.is_empty());
            let (Some(config), true, 1, None) = (config, ours, rows.len(), found.as_ref()) else {
                return Err("DeepSeek Harness 的 agentnave 条目需手动配置".into());
            };
            let start = item.start + row.start;
            found = Some(DshItem {
                full: item.clone(),
                config: start + config.body.start..start + config.body.end,
                indent: indent + 2,
            });
        }
    }
    Ok(found)
}
