//! Remove nonspoken Markdown before either summary or full playback.
/// Without a summary, a long message is read only as far as its opening.
const EXCERPT_CHARS: usize = 100;

fn path_or_url(token: &str) -> bool {
    if token.contains("://")
        || token.starts_with("www.")
        || token.starts_with('/')
        || token.starts_with("~/")
        || token.starts_with("./")
        || token.starts_with("../")
        || token.contains('\\')
    {
        return true;
    }
    let parts: Vec<_> = token.trim_end_matches(['.', '!', '?']).split('/').collect();
    // Two-part uppercase abbreviations (CI/CD, HTTP/2) and fractions are spoken prose.
    let abbreviation = parts.len() == 2
        && parts.iter().all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        });
    parts.len() >= 2 && parts.iter().all(|part| !part.is_empty()) && !abbreviation
}
fn strip_links(line: &str) -> String {
    let mut out = String::new();
    let mut rest = line;
    while let Some(at) = rest.find("](") {
        let (before, after) = rest.split_at(at);
        let Some(end) = after.find(')') else {
            break;
        };
        out.push_str(before);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}
fn prose(line: &str) -> String {
    let line = strip_links(line);
    let mut out = String::new();
    let mut rest = line.as_str();
    while let Some(c) = rest.chars().next() {
        rest = &rest[c.len_utf8()..];
        if c == '<'
            && rest.starts_with(|next: char| next.is_ascii_alphabetic() || next == '/')
            && let Some(end) = rest.find('>')
        {
            rest = &rest[end + 1..];
            continue;
        }
        if !"*`_[]".contains(c) {
            out.push(c);
        }
    }
    let s = out
        .trim()
        .trim_start_matches(['#', '>', '-', '+', '•', ' ']);
    let spoken = spoken_fragments(s)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    match numbered(&spoken) {
        Some((number, rest)) => format!("{number}、{rest}"),
        None => spoken,
    }
}
/// The number and text of a numbered point: `1. `, `1) `, `1）` or `1、`. `3.5 秒` is not one.
fn numbered(line: &str) -> Option<(&str, &str)> {
    let digits = line.bytes().take_while(u8::is_ascii_digit).count();
    if !(1..=2).contains(&digits) {
        return None;
    }
    let (number, rest) = line.split_at(digits);
    [". ", ") ", "）", "、"]
        .iter()
        .find_map(|mark| rest.strip_prefix(mark))
        .map(|rest| (number, rest.trim_start()))
}
fn spoken_fragments(line: &str) -> String {
    let mut output = String::new();
    let mut token = String::new();
    let flush = |output: &mut String, token: &mut String| {
        if !path_or_url(token) {
            output.push_str(token);
        } else {
            // Keep sentence punctuation following an otherwise silent path or URL.
            let ending = token.trim_end_matches(['.', '!', '?']);
            output.push_str(&token[ending.len()..]);
        }
        token.clear();
    };
    for c in line.chars() {
        if !c.is_ascii() || c.is_ascii_whitespace() || ",;!'\"()[]{}".contains(c) {
            flush(&mut output, &mut token);
            output.push(c);
        } else {
            token.push(c);
        }
    }
    flush(&mut output, &mut token);
    output
}
fn table_separator(line: &str) -> bool {
    line.contains('|')
        && line.contains('-')
        && line.chars().all(|c| c.is_whitespace() || "|:-".contains(c))
}

pub fn clean(raw: &str, full: bool) -> String {
    let mut fence: Option<char> = None;
    let mut lines = Vec::new();
    let raw_lines: Vec<_> = raw.lines().collect();
    let mut table = false;
    for (index, line) in raw_lines.iter().enumerate() {
        let line = line.trim();
        if !line.contains('|') {
            table = false;
        }
        if line.contains('|')
            && raw_lines
                .get(index + 1)
                .is_some_and(|next| table_separator(next))
        {
            table = true;
        }
        if line.starts_with("```") || line.starts_with("~~~") {
            let marker = line.chars().next().unwrap();
            if fence == Some(marker) {
                fence = None;
            } else if fence.is_none() {
                fence = Some(marker);
            }
            continue;
        }
        if fence.is_some() || table || (line.starts_with('|') && line.ends_with('|')) {
            continue;
        }
        let line = prose(line);
        if !line.is_empty() {
            lines.push(line);
        }
    }
    let text = lines.join("\n");
    if full { text } else { excerpt(&text) }
}
fn ends_sentence(chars: &[char], index: usize) -> bool {
    "。！？!?".contains(chars[index])
        || (chars[index] == '.' && chars.get(index + 1).is_none_or(|next| next.is_whitespace()))
}
/// The opening of cleaned text: whole paragraphs up to `EXCERPT_CHARS`. When those are under
/// half the limit, as a short heading is, the paragraph that runs past it joins them, cut at its
/// last sentence end within the limit, else at the limit.
pub fn excerpt(text: &str) -> String {
    let mut spoken: Vec<String> = Vec::new();
    let mut total = 0;
    for paragraph in text.lines() {
        let count = paragraph.chars().count();
        if total + count <= EXCERPT_CHARS {
            total += count;
            spoken.push(paragraph.to_owned());
            continue;
        }
        if total < EXCERPT_CHARS / 2 {
            let chars: Vec<char> = paragraph.chars().collect();
            let scan = EXCERPT_CHARS - total;
            let end = (0..scan)
                .rfind(|index| ends_sentence(&chars, *index))
                .map_or(scan, |index| index + 1);
            spoken.push(chars[..end].iter().collect());
        }
        break;
    }
    spoken.join("\n").trim().to_string()
}
