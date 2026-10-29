//! Host configuration codecs. Edits are scoped to one server, never project files.
use serde_json::{Value, json};
use std::ops::Range;

#[derive(Clone, Copy, Debug)]
pub enum Format {
    Json,
    Toml,
    Yaml,
    /// A dsh `cordis.patch.yml`: a top-level YAML list whose `- insert:` item adds the MCP row.
    DshPatch,
}

pub struct Document {
    pub raw: String,
    pub entry: Option<Value>,
    pub rest: Value,
    format: Format,
    key: &'static str,
}

impl Document {
    pub fn parse(raw: &str, format: Format, key: &'static str) -> Result<Self, String> {
        let (entry, rest) = match format {
            Format::Json | Format::Toml => {
                let mut all = if raw.trim().is_empty() {
                    json!({})
                } else if matches!(format, Format::Json) {
                    let value =
                        serde_json::from_str(&clean_json(raw)?).map_err(|_| "JSON 配置解析失败")?;
                    // Duplicate keys would make a scoped edit ambiguous even if a permissive
                    // JSON parser accepts their last occurrence.
                    let root = json_fields(raw)?;
                    if let Some(parent) = root.iter().find(|f| f.0 == key) {
                        let servers = &raw[parent.1.clone()];
                        let fields = json_fields(servers)?;
                        if let Some(server) = fields.iter().find(|f| f.0 == "agentnave") {
                            let entry = &servers[server.1.clone()];
                            for field in json_fields(entry)? {
                                if matches!(field.0.as_str(), "env" | "environment") {
                                    json_fields(&entry[field.1])?;
                                }
                            }
                        }
                    }
                    value
                } else {
                    let value: toml::Value =
                        toml::from_str(raw).map_err(|_| "TOML 配置解析失败")?;
                    serde_json::to_value(value).map_err(|_| "TOML 配置转换失败")?
                };
                if !all.is_object() {
                    return Err("配置根必须为对象".into());
                }
                let entry = match all.get_mut(key) {
                    Some(servers) => servers
                        .as_object_mut()
                        .ok_or("MCP 配置必须为对象")?
                        .remove("agentnave"),
                    None => None,
                };
                // An absent and an empty server container are semantically equivalent.
                if all
                    .get(key)
                    .is_some_and(|v| v.as_object().is_some_and(|o| o.is_empty()))
                {
                    all.as_object_mut().unwrap().remove(key);
                }
                (entry, all)
            }
            Format::Yaml => {
                let root = yaml_fields(raw, 0)?;
                if let Some(parent) = root.iter().find(|f| f.key == key) {
                    if !parent.value.is_empty() && parent.value != "{}" {
                        return Err("Hermes mcp_servers 需使用普通 YAML 映射块".into());
                    }
                    let block = &raw[parent.body.clone()];
                    let fields = yaml_fields(block, yaml_indent(block, parent.indent + 2))?;
                    if let Some(field) = fields.iter().find(|f| f.key == "agentnave") {
                        if !field.value.is_empty() {
                            return Err("Hermes agentnave 需使用普通 YAML 映射块".into());
                        }
                        let entry = yaml_entry(
                            &block[field.body.clone()],
                            yaml_indent(&block[field.body.clone()], field.indent + 2),
                        )?;
                        let mut rest = raw.to_owned();
                        let start = parent.body.start + field.full.start;
                        let end = parent.body.start + field.full.end;
                        rest.replace_range(start..end, "");
                        (Some(entry), json!(rest))
                    } else {
                        (None, json!(raw))
                    }
                } else {
                    (None, json!(raw))
                }
            }
            Format::DshPatch => match dsh_item(raw)? {
                Some(item) => {
                    let config = &raw[item.config.clone()];
                    let entry = yaml_entry(config, yaml_indent(config, item.indent + 2))?;
                    let mut rest = raw.to_owned();
                    rest.replace_range(item.full, "");
                    (Some(entry), json!(rest))
                }
                None => (None, json!(raw)),
            },
        };
        if entry.as_ref().is_some_and(|e| !e.is_object()) {
            return Err("agentnave 条目必须为对象".into());
        }
        Ok(Self {
            raw: raw.into(),
            entry,
            rest,
            format,
            key,
        })
    }

    pub fn render(&self, entry: Option<&Value>) -> Result<String, String> {
        match self.format {
            Format::Json => {
                let raw = if self.raw.trim().is_empty() {
                    "{}"
                } else {
                    &self.raw
                };
                if let Some(parent) = json_fields(raw)?.into_iter().find(|f| f.0 == self.key) {
                    let indent = line_indent(raw, parent.2.start).unwrap_or("");
                    let changed = json_set(
                        &raw[parent.1.clone()],
                        "agentnave",
                        entry,
                        indent,
                        json_indent(raw),
                        newline(raw),
                    )?;
                    let mut out = raw.to_owned();
                    out.replace_range(parent.1, &changed);
                    Ok(out)
                } else if let Some(entry) = entry {
                    json_set(
                        raw,
                        self.key,
                        Some(&json!({"agentnave": entry})),
                        "",
                        json_indent(raw),
                        newline(raw),
                    )
                } else {
                    Ok(raw.into())
                }
            }
            Format::Toml => {
                let mut doc: toml_edit::DocumentMut =
                    self.raw.parse().map_err(|_| "TOML 解析失败")?;
                if let Some(entry) = entry {
                    if doc.get(self.key).is_none() {
                        doc[self.key] = toml_edit::Item::Table(toml_edit::Table::new());
                    }
                    let inline = doc[self.key].is_inline_table();
                    let servers = doc[self.key]
                        .as_table_like_mut()
                        .ok_or("TOML MCP 配置无效")?;
                    if let Some(current) = servers.get_mut("agentnave") {
                        toml_patch(current, self.entry.as_ref(), entry)?;
                    } else {
                        servers.insert("agentnave", toml_item(entry, inline)?);
                    }
                } else if let Some(servers) =
                    doc.get_mut(self.key).and_then(|v| v.as_table_like_mut())
                {
                    servers.remove("agentnave");
                }
                // toml_edit emits LF separators. Restore the file's CRLF style after editing.
                Ok(doc
                    .to_string()
                    .replace("\r\n", "\n")
                    .replace('\n', newline(&self.raw)))
            }
            Format::Yaml => self.render_yaml(entry),
            Format::DshPatch => self.render_dsh(entry),
        }
    }

    fn render_dsh(&self, desired: Option<&Value>) -> Result<String, String> {
        let eol = newline(&self.raw);
        let mut out = self.raw.clone();
        match (dsh_item(&self.raw)?, desired) {
            (Some(item), Some(desired)) => {
                let config = &self.raw[item.config.clone()];
                let changed = yaml_patch(
                    config,
                    yaml_indent(config, item.indent + 2),
                    self.entry.as_ref().unwrap(),
                    desired,
                    yaml_step(&self.raw),
                    eol,
                )?;
                out.replace_range(item.config, &changed);
            }
            (Some(item), None) => {
                out.replace_range(item.full, "");
                // dsh refuses to boot on a patch file that is not a list, even an empty one.
                if dsh_items(&out)?.is_empty() && !dsh_lines(&out).any(|(_, l)| dsh_empty(l)) {
                    if !out.is_empty() && !out.ends_with('\n') {
                        out.push_str(eol);
                    }
                    out.push_str(&format!("[]{eol}"));
                }
            }
            (None, Some(entry)) => {
                let mut block = format!(
                    "- insert:{eol}    - id: agentnave{eol}      name: \"{DSH_MCP_CLIENT}\"{eol}      config:{eol}"
                );
                for (key, value) in entry.as_object().unwrap() {
                    block.push_str(&yaml_line(key, value, 8, 2, eol));
                }
                if let Some((offset, line)) = dsh_lines(&self.raw).find(|(_, l)| dsh_empty(l)) {
                    // Expand the empty flow list before adding a block item.
                    out.replace_range(offset..offset + line.len(), &block);
                } else {
                    if !out.is_empty() && !out.ends_with('\n') {
                        out.push_str(eol);
                    }
                    out.push_str(&block);
                }
            }
            (None, None) => {}
        }
        Ok(out)
    }

    fn render_yaml(&self, desired: Option<&Value>) -> Result<String, String> {
        let eol = newline(&self.raw);
        let mut out = self.raw.clone();
        let root = yaml_fields(&out, 0)?;
        if let Some(parent) = root.iter().find(|f| f.key == self.key) {
            let block = &out[parent.body.clone()];
            let fields = yaml_fields(block, yaml_indent(block, parent.indent + 2))?;
            if let Some(field) = fields.iter().find(|f| f.key == "agentnave") {
                let start = parent.body.start + field.full.start;
                let end = parent.body.start + field.full.end;
                let replacement = if let Some(desired) = desired {
                    let body = &block[field.body.clone()];
                    let indent = yaml_indent(body, field.indent + 2);
                    let changed = yaml_patch(
                        body,
                        indent,
                        self.entry.as_ref().unwrap(),
                        desired,
                        yaml_step(&self.raw),
                        eol,
                    )?;
                    yaml_block(&block[field.full.start..field.body.start], &changed, eol)
                } else {
                    String::new()
                };
                out.replace_range(start..end, &replacement);
            } else if let Some(entry) = desired {
                // Expand an empty flow map before adding a block.
                if parent.value == "{}" {
                    out.replace_range(
                        parent.full.clone(),
                        &format!(
                            "mcp_servers:{eol}{}",
                            yaml_server(
                                entry,
                                yaml_indent(block, parent.indent + 2),
                                yaml_step(&self.raw),
                                eol,
                            )
                        ),
                    );
                } else {
                    let separator = if out[..parent.body.end].ends_with('\n') {
                        ""
                    } else {
                        eol
                    };
                    out.insert_str(
                        parent.body.end,
                        &format!(
                            "{separator}{}",
                            yaml_server(
                                entry,
                                yaml_indent(block, parent.indent + 2),
                                yaml_step(&self.raw),
                                eol,
                            )
                        ),
                    );
                }
            }
        } else if let Some(entry) = desired {
            if !out.is_empty() && !out.ends_with('\n') {
                out.push_str(eol);
            }
            out.push_str(&format!(
                "mcp_servers:{eol}{}",
                yaml_server(entry, 2, 2, eol)
            ));
        }
        Ok(out)
    }

    pub fn verify(&self, written: &str, desired: Option<&Value>) -> Result<(), String> {
        let after = Self::parse(written, self.format, self.key)?;
        if after.entry.as_ref() != desired {
            return Err("写后条目校验失败".into());
        }
        if matches!(self.format, Format::Yaml | Format::DshPatch) {
            // Compare exact non-target bytes, allowing only a newly created empty container.
            let expected = self.render(desired)?;
            if written != expected {
                return Err("写后校验发现其他 YAML 内容变化".into());
            }
        } else if after.rest != self.rest {
            return Err("写后校验发现其他配置变化".into());
        }
        Ok(())
    }
}

fn toml_item(value: &Value, inline: bool) -> Result<toml_edit::Item, String> {
    if let Some(object) = value.as_object() {
        let mut table = toml_edit::Table::new();
        for (key, value) in object {
            table.insert(key, toml_item(value, inline)?);
        }
        let mut item = toml_edit::Item::Table(table);
        if inline {
            item.make_value();
        }
        Ok(item)
    } else {
        let encoded = toml::to_string(&json!({"value": value})).map_err(|_| "TOML 编码失败")?;
        let mut encoded: toml_edit::DocumentMut = encoded.parse().map_err(|_| "TOML 编码失败")?;
        Ok(encoded.remove("value").ok_or("TOML 值编码失败")?)
    }
}

fn toml_patch(
    item: &mut toml_edit::Item,
    before: Option<&Value>,
    desired: &Value,
) -> Result<(), String> {
    if before == Some(desired) {
        return Ok(());
    }
    let inline = item.is_inline_table();
    if let (Some(table), Some(object)) = (item.as_table_like_mut(), desired.as_object()) {
        if let Some(old) = before.and_then(Value::as_object) {
            for key in old.keys().filter(|k| !object.contains_key(*k)) {
                table.remove(key);
            }
        }
        for (key, value) in object {
            if let Some(current) = table.get_mut(key) {
                toml_patch(current, before.and_then(|v| v.get(key)), value)?;
            } else {
                table.insert(key, toml_item(value, inline)?);
            }
        }
    } else {
        let mut replacement = toml_item(desired, inline)?;
        if let (Some(old), Some(new)) = (item.as_value(), replacement.as_value_mut()) {
            *new.decor_mut() = old.decor().clone();
        }
        *item = replacement;
    }
    Ok(())
}

fn newline(raw: &str) -> &'static str {
    if raw.contains("\r\n") { "\r\n" } else { "\n" }
}
fn yaml_block(header: &str, body: &str, eol: &str) -> String {
    let separator = if !header.ends_with('\n') && !body.is_empty() {
        eol
    } else {
        ""
    };
    format!("{header}{separator}{body}")
}
fn yaml_indent(raw: &str, fallback: usize) -> usize {
    raw.lines()
        .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
        .map(|l| l.len() - l.trim_start_matches(' ').len())
        .min()
        .unwrap_or(fallback)
}
fn yaml_step(raw: &str) -> usize {
    raw.lines()
        .map(|l| l.len() - l.trim_start_matches(' ').len())
        .filter(|n| *n > 0)
        .min()
        .unwrap_or(2)
}
fn yaml_patch(
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

fn yaml_line(key: &str, value: &Value, indent: usize, step: usize, eol: &str) -> String {
    if let Some(object) = value.as_object().filter(|o| !o.is_empty()) {
        let mut out = format!("{}{key}:{eol}", " ".repeat(indent));
        for (key, value) in object {
            out.push_str(&yaml_line(key, value, indent + step, step, eol));
        }
        return out;
    }
    format!("{}{key}: {}{eol}", " ".repeat(indent), value)
}
fn yaml_server(entry: &Value, indent: usize, step: usize, eol: &str) -> String {
    let mut out = format!("{}agentnave:{eol}", " ".repeat(indent));
    for (key, value) in entry.as_object().unwrap() {
        out.push_str(&yaml_line(key, value, indent + step, step, eol));
    }
    out
}

struct YamlField {
    key: String,
    value: String,
    indent: usize,
    full: Range<usize>,
    body: Range<usize>,
}

/// Only ordinary block mappings at the traversed levels. Unknown field bodies stay opaque.
/// Anchors, aliases, directives and duplicate mapping keys are rejected rather than rewritten.
fn yaml_fields(raw: &str, indent: usize) -> Result<Vec<YamlField>, String> {
    let mut fields: Vec<YamlField> = Vec::new();
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
            fields.push(YamlField {
                key: key.into(),
                value: yaml_value_text(value)?.to_owned(),
                indent,
                full: offset..raw.len(),
                body: offset + line.len()..raw.len(),
            });
        } else if fields.is_empty() {
            return Err("YAML 缩进无效".into());
        }
        offset += line.len();
    }
    Ok(fields)
}

fn yaml_value_text(text: &str) -> Result<&str, String> {
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

fn yaml_scalar(text: &str) -> Result<Value, String> {
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

fn yaml_entry(raw: &str, indent: usize) -> Result<Value, String> {
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

const DSH_MCP_CLIENT: &str = "@deepseek-ai/dsh-mcp-client";

/// AgentNave's item in a dsh patch list and the `config` body of the row it inserts.
struct DshItem {
    full: Range<usize>,
    config: Range<usize>,
    indent: usize,
}

/// Column-0 lines that are not blank, comments included, with their byte offsets.
fn dsh_lines(raw: &str) -> impl Iterator<Item = (usize, &str)> {
    raw.split_inclusive('\n')
        .scan(0, |offset, line| {
            let start = *offset;
            *offset += line.len();
            Some((start, line))
        })
        .filter(|(_, line)| !line.starts_with([' ', '\t']) && !line.trim().is_empty())
}

fn dsh_empty(line: &str) -> bool {
    yaml_value_text(line).is_ok_and(|text| text == "[]")
}

/// Top-level list items, each up to the next column-0 line. Only block lists are edited: any
/// other column-0 content than items, comments and an empty `[]` needs manual configuration.
fn dsh_items(raw: &str) -> Result<Vec<Range<usize>>, String> {
    let mut items: Vec<Range<usize>> = Vec::new();
    let lines: Vec<_> = dsh_lines(raw).collect();
    for (i, (offset, line)) in lines.iter().enumerate() {
        if line.starts_with("- ") || line.trim_end() == "-" {
            items.push(*offset..lines.get(i + 1).map_or(raw.len(), |next| next.0));
        } else if !line.starts_with('#') && !dsh_empty(line) {
            return Err("DeepSeek Harness 补丁需为块式 YAML 列表，需手动配置".into());
        }
    }
    Ok(items)
}

/// The `- insert:` item whose single row is `id: agentnave`. Other items stay opaque, so their
/// `!!js` expressions are never parsed; a row sharing an item with others is left to the user.
fn dsh_item(raw: &str) -> Result<Option<DshItem>, String> {
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
            let config = fields
                .iter()
                .find(|f| f.key == "config" && f.value.is_empty());
            let (Some(config), 1, None) = (config, rows.len(), found.as_ref()) else {
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

/// Remove JSONC comments/trailing commas while retaining byte offsets for surgical edits.
fn clean_json(raw: &str) -> Result<String, String> {
    let mut bytes = raw.as_bytes().to_vec();
    let mut i = 0;
    let mut string = false;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if string => {
                i += 2;
                continue;
            }
            b'"' => string = !string,
            b'/' if !string && bytes.get(i + 1) == Some(&b'/') => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    bytes[i] = b' ';
                    i += 1;
                }
                continue;
            }
            b'/' if !string && bytes.get(i + 1) == Some(&b'*') => {
                bytes[i] = b' ';
                bytes[i + 1] = b' ';
                i += 2;
                while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    if bytes[i] != b'\n' {
                        bytes[i] = b' ';
                    }
                    i += 1;
                }
                if i + 1 >= bytes.len() {
                    return Err("JSONC 注释未闭合".into());
                }
                bytes[i] = b' ';
                bytes[i + 1] = b' ';
                i += 2;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    string = false;
    i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if string => {
                i += 2;
                continue;
            }
            b'"' => string = !string,
            b',' if !string => {
                let next = bytes[i + 1..].iter().find(|b| !b.is_ascii_whitespace());
                if matches!(next, Some(b'}' | b']')) {
                    bytes[i] = b' ';
                }
            }
            _ => {}
        }
        i += 1;
    }
    String::from_utf8(bytes).map_err(|_| "JSONC 编码无效".into())
}

type JsonField = (String, Range<usize>, Range<usize>);
fn json_fields(raw: &str) -> Result<Vec<JsonField>, String> {
    let clean = clean_json(raw)?;
    let b = clean.as_bytes();
    let mut i = b
        .iter()
        .position(|c| !c.is_ascii_whitespace())
        .ok_or("JSON 对象为空")?;
    if b[i] != b'{' {
        return Err("JSON MCP 配置必须为对象".into());
    }
    i += 1;
    let mut out = Vec::new();
    loop {
        while b
            .get(i)
            .is_some_and(|c| c.is_ascii_whitespace() || *c == b',')
        {
            i += 1;
        }
        if b.get(i) == Some(&b'}') {
            break;
        }
        let start = i;
        if b.get(i) != Some(&b'"') {
            return Err("JSON 键无效".into());
        }
        i += 1;
        while i < b.len() && b[i] != b'"' {
            if b[i] == b'\\' {
                i += 1;
            }
            i += 1;
        }
        i += 1;
        let key = serde_json::from_str(&clean[start..i]).map_err(|_| "JSON 键无效")?;
        if out.iter().any(|f: &JsonField| f.0 == key) {
            return Err("JSON 键重复，需手动处理".into());
        }
        while b.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        if b.get(i) != Some(&b':') {
            return Err("JSON 缺少冒号".into());
        }
        i += 1;
        while b.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        let value_start = i;
        let mut stream = serde_json::Deserializer::from_str(&clean[i..]).into_iter::<Value>();
        stream
            .next()
            .ok_or("JSON 值缺失")?
            .map_err(|_| "JSON 值无效")?;
        i += stream.byte_offset();
        out.push((key, value_start..i, start..i));
    }
    Ok(out)
}

fn line_indent(raw: &str, offset: usize) -> Option<&str> {
    let start = raw[..offset].rfind('\n').map_or(0, |i| i + 1);
    let prefix = &raw[start..offset];
    prefix
        .chars()
        .all(|c| c == ' ' || c == '\t')
        .then_some(prefix)
}
fn json_indent(raw: &str) -> &str {
    raw.lines()
        .filter_map(|line| {
            let trimmed = line.trim_start_matches([' ', '\t']);
            let indent = &line[..line.len() - trimmed.len()];
            (!indent.is_empty() && trimmed.starts_with('"')).then_some(indent)
        })
        .min_by_key(|s| s.len())
        .unwrap_or("  ")
}
fn json_encode(value: &Value, indent: &str, step: &str, eol: &str) -> String {
    serde_json::to_string_pretty(value)
        .unwrap()
        .lines()
        .enumerate()
        .map(|(i, line)| {
            let content = line.trim_start_matches(' ');
            let depth = (line.len() - content.len()) / 2;
            format!(
                "{}{}{content}",
                if i == 0 { "" } else { indent },
                step.repeat(depth)
            )
        })
        .collect::<Vec<_>>()
        .join(eol)
}
fn json_patch(
    raw: &str,
    desired: &Value,
    indent: &str,
    step: &str,
    eol: &str,
) -> Result<String, String> {
    let original: Value = serde_json::from_str(&clean_json(raw)?).map_err(|_| "JSON 解析失败")?;
    if original == *desired {
        return Ok(raw.into());
    }
    if let (Some(old), Some(new)) = (original.as_object(), desired.as_object()) {
        let mut changed = raw.to_owned();
        for (key, _, _) in json_fields(raw)?.iter().rev() {
            if old.get(key) != new.get(key) {
                // Edit by key again because removing siblings can change byte offsets.
                changed = json_set(&changed, key, new.get(key), indent, step, eol)?;
            }
        }
        for (key, value) in new {
            if !old.contains_key(key) {
                changed = json_set(&changed, key, Some(value), indent, step, eol)?;
            }
        }
        Ok(changed)
    } else {
        Ok(json_encode(desired, indent, step, eol))
    }
}
fn json_set(
    raw: &str,
    key: &str,
    value: Option<&Value>,
    indent: &str,
    step: &str,
    eol: &str,
) -> Result<String, String> {
    let fields = json_fields(raw)?;
    let mut out = raw.to_owned();
    if let Some((_, range, full)) = fields.iter().find(|f| f.0 == key) {
        if let Some(value) = value {
            let field_indent = line_indent(raw, full.start).unwrap_or(indent);
            let changed = json_patch(&raw[range.clone()], value, field_indent, step, eol)?;
            out.replace_range(range.clone(), &changed);
        } else {
            let clean = clean_json(raw)?;
            let mut range = full.clone();
            if let Some(next) = fields.iter().find(|f| f.2.start > full.start) {
                range.end = next.2.start;
            } else if let Some(previous) = fields.iter().rev().find(|f| f.2.end < full.start) {
                range.start = previous.2.end;
            } else {
                // The only member: drop its line and a JSONC trailing comma, but keep the
                // closing brace on its own line at its own indentation.
                let bytes = clean.as_bytes();
                while range.start > 0 && bytes[range.start - 1].is_ascii_whitespace() {
                    range.start -= 1;
                }
                let mut end = range.end;
                while bytes
                    .get(end)
                    .is_some_and(|c| c.is_ascii_whitespace() || *c == b',')
                {
                    if bytes[end] == b'\n' {
                        range.end = end;
                    }
                    end += 1;
                }
                if !clean[range.end..end].contains('\n') {
                    range.end = end;
                }
            }
            out.replace_range(range, "");
        }
    } else if let Some(value) = value {
        let clean = clean_json(raw)?;
        let end = clean.rfind('}').ok_or("JSON 对象未闭合")?;
        let field_indent = fields
            .first()
            .and_then(|f| line_indent(raw, f.2.start))
            .map(str::to_owned)
            .unwrap_or_else(|| format!("{indent}{step}"));
        let encoded = json_encode(value, &field_indent, step, eol);
        if let Some(first) = fields.first().filter(|f| raw[..f.2.start].contains('\n')) {
            // Prepend a new member: existing members keep their order and even the old last
            // line's comma stays untouched. JSON requires a separator; put it on our new line.
            let start = first.2.start - line_indent(raw, first.2.start).unwrap_or("").len();
            out.insert_str(start, &format!("{field_indent}{key:?}: {encoded},{eol}"));
        } else if fields.is_empty() {
            let closing = line_indent(raw, end).map(|i| end - i.len()).unwrap_or(end);
            let before = if raw[..closing].ends_with('\n') {
                ""
            } else {
                eol
            };
            let after = if closing == end { indent } else { "" };
            out.insert_str(
                closing,
                &format!("{before}{field_indent}{key:?}: {encoded}{eol}{after}"),
            );
        } else {
            let tail = clean_json(&raw[fields.last().unwrap().2.end..end])?;
            let comma = if tail.contains(',') { "" } else { "," };
            out.insert_str(end, &format!("{comma} {key:?}: {encoded}"));
        }
    }
    Ok(out)
}
