//! One line of plain text summarising a member's output or failure.
use super::failure;

fn emphasis_end(text: &str, marker: &str) -> Option<usize> {
    let mut rest = text;
    while !rest.is_empty() {
        if rest.starts_with('`') {
            let size = rest.bytes().take_while(|b| *b == b'`').count();
            if let Some(end) = rest[size..].find(&rest[..size]) {
                rest = &rest[size + end + size..];
                continue;
            }
        }
        if rest.starts_with(marker) {
            return Some(text.len() - rest.len());
        }
        rest = &rest[rest.chars().next()?.len_utf8()..];
    }
    None
}

fn inline_text(mut rest: &str) -> String {
    let mut result = String::new();
    while let Some(first) = rest.chars().next() {
        if first == '`' {
            let size = rest.bytes().take_while(|b| *b == b'`').count();
            let marker = &rest[..size];
            if let Some(end) = rest[size..].find(marker) {
                result.push_str(&rest[size..size + end]);
                rest = &rest[size + end + size..];
                continue;
            }
        }
        let marker = if rest.starts_with("**") { "**" } else { "__" };
        if rest.starts_with(marker)
            && let Some(end) = emphasis_end(&rest[2..], marker).filter(|end| *end > 0)
        {
            let after = &rest[2 + end + 2..];
            // Underscores inside names are literal, rather than emphasis delimiters.
            let in_word = marker == "__"
                && (result.ends_with(char::is_alphanumeric)
                    || after.starts_with(char::is_alphanumeric));
            if !in_word {
                result.push_str(&inline_text(&rest[2..2 + end]));
                rest = after;
                continue;
            }
        }
        result.push(first);
        rest = &rest[first.len_utf8()..];
    }
    result
}

fn plain(line: &str) -> String {
    let mut line = line.trim();
    loop {
        let heading = line.bytes().take_while(|b| *b == b'#').count();
        let numbered = line.trim_start_matches(|c: char| c.is_ascii_digit());
        let next = if (1..=6).contains(&heading) && line[heading..].starts_with(char::is_whitespace)
        {
            Some(&line[heading..])
        } else {
            line.strip_prefix("> ")
                .or_else(|| line.strip_prefix("- "))
                .or_else(|| line.strip_prefix("+ "))
                .or_else(|| line.strip_prefix("* "))
                .or_else(|| {
                    if numbered.len() < line.len() {
                        numbered
                            .strip_prefix(". ")
                            .or_else(|| numbered.strip_prefix(") "))
                    } else {
                        None
                    }
                })
        };
        let Some(next) = next else {
            break;
        };
        line = next.trim_start();
    }
    inline_text(line).trim().chars().take(120).collect()
}

pub(super) fn line(status: &str, output: &str, error: &str) -> String {
    if status == "review" {
        return "待审核：请在 boss 中决定".into();
    }
    let lines = || output.lines().map(plain).filter(|s| !s.is_empty());
    match status {
        "running" => lines().next_back().unwrap_or_default(),
        "succeeded" => lines().next().unwrap_or_default(),
        // A cancel's reason only repeats the status.
        "cancelled" => failure(status).into(),
        _ => {
            let error = error
                .lines()
                .map(plain)
                .find(|s| !s.is_empty())
                .unwrap_or_default();
            format!(
                "{}{}{}",
                failure(status),
                if error.is_empty() { "" } else { "：" },
                error
            )
            .chars()
            .take(120)
            .collect()
        }
    }
}
