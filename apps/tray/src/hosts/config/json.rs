use super::*;

/// Remove JSONC comments/trailing commas while retaining byte offsets for surgical edits.
pub(super) fn clean_json(raw: &str) -> Result<String, String> {
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

pub(super) type JsonField = (String, Range<usize>, Range<usize>);
pub(super) fn json_fields(raw: &str) -> Result<Vec<JsonField>, String> {
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

pub(super) fn line_indent(raw: &str, offset: usize) -> Option<&str> {
    let start = raw[..offset].rfind('\n').map_or(0, |i| i + 1);
    let prefix = &raw[start..offset];
    prefix
        .chars()
        .all(|c| c == ' ' || c == '\t')
        .then_some(prefix)
}
pub(super) fn json_indent(raw: &str) -> &str {
    raw.lines()
        .filter_map(|line| {
            let trimmed = line.trim_start_matches([' ', '\t']);
            let indent = &line[..line.len() - trimmed.len()];
            (!indent.is_empty() && trimmed.starts_with('"')).then_some(indent)
        })
        .min_by_key(|s| s.len())
        .unwrap_or("  ")
}
pub(super) fn json_encode(value: &Value, indent: &str, step: &str, eol: &str) -> String {
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
pub(super) fn json_patch(
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
pub(super) fn json_set(
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

pub(super) fn validate_json_servers(raw: &str, key: &str) -> Result<(), String> {
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
    Ok(())
}
