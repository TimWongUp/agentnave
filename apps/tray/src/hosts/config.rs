//! Host configuration codecs. Edits are scoped to one server, never project files.
mod json;
mod yaml;
use self::json::*;
use self::yaml::*;

use serde_json::{Value, json};
use std::ops::Range;

#[derive(Clone, Copy, Debug)]
pub enum Format {
    Json,
    Zcode,
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
        if matches!(format, Format::Zcode) {
            return Self::parse_zcode(raw, format, key);
        }
        let (entry, rest) = match format {
            Format::Zcode => unreachable!(),
            Format::Json | Format::Toml => {
                let mut all = if raw.trim().is_empty() {
                    json!({})
                } else if matches!(format, Format::Json) {
                    let value =
                        serde_json::from_str(&clean_json(raw)?).map_err(|_| "JSON 配置解析失败")?;
                    validate_json_servers(raw, key)?;
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
                    let block_indent = yaml_indent(block, parent.indent + 2);
                    if block_indent <= parent.indent {
                        return Err("Hermes mcp_servers 需使用普通 YAML 映射块".into());
                    }
                    let fields = yaml_fields(block, block_indent)?;
                    if let Some(field) = fields.iter().find(|f| f.key == "agentnave") {
                        if !field.value.is_empty() {
                            return Err("Hermes agentnave 需使用普通 YAML 映射块".into());
                        }
                        let entry_body = &block[field.body.clone()];
                        let entry_indent = yaml_indent(entry_body, field.indent + 2);
                        if entry_indent <= field.indent {
                            return Err("Hermes agentnave 需使用普通 YAML 映射块".into());
                        }
                        let entry = yaml_entry(entry_body, entry_indent)?;
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

    fn parse_zcode(raw: &str, format: Format, key: &'static str) -> Result<Self, String> {
        let raw_object = if raw.trim().is_empty() { "{}" } else { raw };
        let mut rest: Value =
            serde_json::from_str(&clean_json(raw_object)?).map_err(|_| "JSON 配置解析失败")?;
        let fields = json_fields(raw_object)?;
        let nested = fields.iter().find(|f| f.0 == "mcp");
        let document = Self::parse(
            nested.map_or("{}", |f| &raw_object[f.1.clone()]),
            Format::Json,
            "servers",
        )?;
        if document.rest.as_object().is_some_and(|o| o.is_empty()) {
            rest.as_object_mut()
                .ok_or("配置根必须为对象")?
                .remove("mcp");
        } else {
            rest["mcp"] = document.rest;
        }
        Ok(Self {
            raw: raw.into(),
            entry: document.entry,
            rest,
            format,
            key,
        })
    }

    pub fn render(&self, entry: Option<&Value>) -> Result<String, String> {
        match self.format {
            Format::Zcode => {
                let raw = if self.raw.trim().is_empty() {
                    "{}"
                } else {
                    &self.raw
                };
                if let Some(parent) = json_fields(raw)?.into_iter().find(|f| f.0 == "mcp") {
                    let nested = Self::parse(&raw[parent.1.clone()], Format::Json, "servers")?;
                    let changed = nested.render(entry)?;
                    let mut out = raw.to_owned();
                    out.replace_range(parent.1, &changed);
                    Ok(out)
                } else if let Some(entry) = entry {
                    json_set(
                        raw,
                        "mcp",
                        Some(&json!({"servers":{"agentnave":entry}})),
                        "",
                        json_indent(raw),
                        newline(raw),
                    )
                } else {
                    Ok(raw.into())
                }
            }
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
                    // Expand the empty flow list before adding a block item, keeping its comment.
                    if let Some(comment) = line.find('#').map(|i| line[i..].trim_end()) {
                        block.insert_str(0, &format!("{comment}{eol}"));
                    }
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
        }
        if matches!(self.format, Format::DshPatch) {
            // Independent of render: other items and comments survive, only `[]` comes or goes.
            if dsh_rest(&after.rest) != dsh_rest(&self.rest) {
                return Err("写后校验发现其他补丁变化".into());
            }
        } else if !matches!(self.format, Format::Yaml) && after.rest != self.rest {
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
