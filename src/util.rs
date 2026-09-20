use crate::model::Container;

pub fn sort_containers(list: Vec<&Container>) -> Vec<&Container> {
    let mut v = list;
    v.sort_by_key(|c| (c.state != "running", c.name.clone()));
    v
}

/// 1234 -> "1.2K", 5 -> "5B", 0 -> "-", 730 -> "730B" (below 1024 stays in bytes).
pub fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "K", "M", "G", "T"];
    if n == 0 {
        return "-".into();
    }
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{n}B")
    } else {
        format!("{v:.1}{}", UNITS[u])
    }
}

/// Truncate a string to fit `width` display columns, appending an ellipsis if
/// it does not fit. Returns a String no wider than `width`.
pub fn truncate(s: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    use unicode_width::UnicodeWidthStr;
    if s.width() <= width {
        return s.to_string();
    }
    use unicode_width::UnicodeWidthChar;
    let ell_width = "…".width();
    let target = width.saturating_sub(ell_width);
    let mut out = String::new();
    let mut w = 0;
    for ch in s.chars() {
        let cw = ch.width().unwrap_or(0);
        if w + cw > target {
            break;
        }
        out.push(ch);
        w += cw;
    }
    if out.width() + ell_width <= width {
        out.push('…');
    }
    out
}

/// Split a command string on whitespace, respecting single and double quotes.
/// `sh -c "apk add jq"` -> ["sh", "-c", "apk add jq"].
pub fn split_command(s: &str) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut has_token = false;
    for ch in s.chars() {
        match quote {
            Some(q) => {
                if ch == q {
                    quote = None;
                } else {
                    cur.push(ch);
                }
            }
            None => match ch {
                '\'' | '"' => {
                    quote = Some(ch);
                    has_token = true;
                }
                c if c.is_whitespace() => {
                    if has_token {
                        args.push(std::mem::take(&mut cur));
                        has_token = false;
                    }
                }
                c => {
                    cur.push(c);
                    has_token = true;
                }
            },
        }
    }
    if has_token {
        args.push(cur);
    }
    args
}

/// Split a list on `;`, trimming whitespace and dropping empty entries.
pub fn split_list(s: &str) -> Vec<String> {
    s.split(';')
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect()
}
