//! Structural merge of MEMORY.md: sections of `- [Title](file.md) — hook` lines keyed by file link.
use std::collections::HashSet;

#[derive(Debug, Clone)]
enum Line {
    Entry { key: String, text: String },
    Other(String),
}

#[derive(Debug, Clone)]
struct Section {
    header: Option<String>,
    lines: Vec<Line>,
}

/// Key of an index line, i.e. the first `](...)` link target
pub fn entry_key(line: &str) -> Option<String> {
    let t = line.trim_start();
    if !t.starts_with("- ") {
        return None;
    }
    let open = t.find("](")? + 2;
    let close = t[open..].find(')')? + open;
    let key = t[open..close].trim();
    if key.is_empty() {
        None
    } else {
        Some(key.to_string())
    }
}

fn parse(text: &str) -> Vec<Section> {
    let mut out = vec![Section {
        header: None,
        lines: vec![],
    }];
    for raw in text.lines() {
        if raw.starts_with("## ") {
            out.push(Section {
                header: Some(raw.to_string()),
                lines: vec![],
            });
            continue;
        }
        let line = match entry_key(raw) {
            Some(key) => Line::Entry {
                key,
                text: raw.to_string(),
            },
            None => Line::Other(raw.to_string()),
        };
        out.last_mut().unwrap().lines.push(line);
    }
    out
}

/// Union of `local` and `remote`; on a shared key take `remote` when its file is in `prefer_remote`.
pub fn merge(local: &str, remote: &str, prefer_remote: &HashSet<String>) -> String {
    let l = parse(local);
    let r = parse(remote);
    let remote_entry = |key: &str| -> Option<String> {
        r.iter()
            .flat_map(|s| s.lines.iter())
            .find_map(|ln| match ln {
                Line::Entry { key: k, text } if k == key => Some(text.clone()),
                _ => None,
            })
    };

    let mut used: HashSet<String> = HashSet::new();
    let mut out: Vec<Section> = vec![];
    for sec in &l {
        let mut lines = vec![];
        for ln in &sec.lines {
            match ln {
                Line::Entry { key, text } => {
                    if !used.insert(key.clone()) {
                        continue;
                    }
                    let text = match (prefer_remote.contains(key), remote_entry(key)) {
                        (true, Some(rt)) => rt,
                        _ => text.clone(),
                    };
                    lines.push(Line::Entry {
                        key: key.clone(),
                        text,
                    });
                }
                Line::Other(t) => lines.push(Line::Other(t.clone())),
            }
        }
        out.push(Section {
            header: sec.header.clone(),
            lines,
        });
    }

    // Remote-only entries go into the same-named section, else a new section at the end
    for sec in &r {
        let mut extra = vec![];
        for ln in &sec.lines {
            if let Line::Entry { key, text } = ln
                && used.insert(key.clone())
            {
                extra.push(Line::Entry {
                    key: key.clone(),
                    text: text.clone(),
                });
            }
        }
        if extra.is_empty() {
            continue;
        }
        match out.iter_mut().find(|s| s.header == sec.header) {
            Some(target) => {
                // Keep trailing blank lines after the appended entries
                let tail_blank = target
                    .lines
                    .iter()
                    .rev()
                    .take_while(|l| matches!(l, Line::Other(t) if t.trim().is_empty()))
                    .count();
                let at = target.lines.len() - tail_blank;
                for (i, e) in extra.into_iter().enumerate() {
                    target.lines.insert(at + i, e);
                }
            }
            None => {
                if let Some(prev) = out.last_mut()
                    && !matches!(prev.lines.last(), Some(Line::Other(t)) if t.trim().is_empty())
                {
                    prev.lines.push(Line::Other(String::new()));
                }
                let mut lines = extra;
                lines.push(Line::Other(String::new()));
                out.push(Section {
                    header: sec.header.clone(),
                    lines,
                });
            }
        }
    }

    let mut s = String::new();
    for sec in out {
        if let Some(h) = sec.header {
            s.push_str(&h);
            s.push('\n');
        }
        for ln in sec.lines {
            match ln {
                Line::Entry { text, .. } | Line::Other(text) => {
                    s.push_str(&text);
                    s.push('\n');
                }
            }
        }
    }
    s
}

/// Index lines present in `after` but not in `before`
pub fn new_lines(before: &str, after: &str) -> Vec<String> {
    let old: HashSet<&str> = before.lines().collect();
    after
        .lines()
        .filter(|l| entry_key(l).is_some() && !old.contains(l))
        .map(|l| l.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn union_keeps_sections_and_dedupes() {
        let a =
            "# Index\n\n## Rules\n- [A](a.md) — a1\n- [B](b.md) — b\n\n## Work\n- [C](c.md) — c\n";
        let b =
            "# Index\n\n## Rules\n- [A](a.md) — a2\n- [D](d.md) — d\n\n## Other\n- [E](e.md) — e\n";
        let mut prefer = HashSet::new();
        prefer.insert("a.md".to_string());
        let m = merge(a, b, &prefer);
        assert!(m.contains("- [A](a.md) — a2"));
        assert!(!m.contains("a1"));
        assert!(m.contains("- [D](d.md) — d\n\n## Work"));
        assert!(m.ends_with("- [C](c.md) — c\n\n## Other\n- [E](e.md) — e\n\n"));
        assert_eq!(m.matches("](a.md)").count(), 1);
    }
}
