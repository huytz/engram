//! Claude Code hook wiring: settings.json edits and the hook entry points.
use crate::config::{Config, claude_settings_path, expand};
use anyhow::{Context, Result};
use serde_json::{Value, json};

const MARK: &str = "engram hook";

fn hook_entry(event: &str, matcher: Option<&str>, timeout: u64, is_async: bool) -> Value {
    let exe = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "engram".into());
    let mut h =
        json!({ "type": "command", "command": format!("{exe} hook {event}"), "timeout": timeout });
    if is_async {
        h["async"] = json!(true);
    }
    let mut entry = json!({ "hooks": [h] });
    if let Some(m) = matcher {
        entry["matcher"] = json!(m);
    }
    entry
}

/// Set `autoMemoryDirectory` in Claude Code's settings.json and add engram hooks, keeping others intact;
/// `plugin` = the engram plugin owns the hooks, so only drop old engram entries
pub fn install(cfg: &Config, plugin: bool) -> Result<()> {
    let path = claude_settings_path();
    let mut root: Value = match std::fs::read_to_string(&path) {
        Ok(s) if !s.trim().is_empty() => {
            serde_json::from_str(&s).with_context(|| format!("parse {}", path.display()))?
        }
        _ => json!({}),
    };
    let obj = root
        .as_object_mut()
        .context("settings.json is not an object")?;

    if let Some(mem) = cfg.collection("memory") {
        obj.insert("autoMemoryDirectory".into(), json!(mem.path));
    }
    let hooks = obj.entry("hooks").or_insert_with(|| json!({}));
    let hooks = hooks.as_object_mut().context("hooks is not an object")?;

    let wanted = [
        ("SessionStart", None, 15, false),
        ("UserPromptSubmit", None, 5, false),
        ("PostToolUse", Some("Edit|Write"), 30, true),
        ("Stop", None, 30, true),
    ];
    for (event, matcher, timeout, is_async) in wanted {
        let list = hooks.entry(event).or_insert_with(|| json!([]));
        let list = list.as_array_mut().context("hook list")?;
        list.retain(|e| !e.to_string().contains(MARK));
        if plugin {
            continue;
        }
        list.push(hook_entry(
            &event.to_lowercase(),
            matcher,
            timeout,
            is_async,
        ));
    }
    hooks.retain(|_, l| l.as_array().is_none_or(|a| !a.is_empty()));
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(&path, serde_json::to_string_pretty(&root)? + "\n")?;
    Ok(())
}

pub fn uninstall() -> Result<()> {
    let path = claude_settings_path();
    let Ok(s) = std::fs::read_to_string(&path) else {
        return Ok(());
    };
    let mut root: Value = serde_json::from_str(&s)?;
    if let Some(hooks) = root.get_mut("hooks").and_then(|h| h.as_object_mut()) {
        let mut empty = vec![];
        for (ev, list) in hooks.iter_mut() {
            if let Some(a) = list.as_array_mut() {
                a.retain(|e| !e.to_string().contains(MARK));
                if a.is_empty() {
                    empty.push(ev.clone());
                }
            }
        }
        for ev in empty {
            hooks.remove(&ev);
        }
    }
    std::fs::write(&path, serde_json::to_string_pretty(&root)? + "\n")?;
    Ok(())
}

/// What a hook learns from its stdin JSON
#[derive(Default)]
pub struct HookInput {
    /// Called by Codex rather than Claude Code
    pub codex: bool,
    /// Files a PostToolUse touched: Claude `file_path`, Codex `apply_patch` headers
    pub files: Vec<std::path::PathBuf>,
}

/// Parse the hook JSON on stdin; empty or a terminal gives the default
pub fn read_input() -> HookInput {
    use std::io::IsTerminal;
    if std::io::stdin().is_terminal() {
        return HookInput::default();
    }
    // Agents close stdin after the JSON; don't hang on a caller that never does
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut s = String::new();
        let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut s);
        let _ = tx.send(s);
    });
    rx.recv_timeout(std::time::Duration::from_millis(300))
        .map(|s| parse_input(&s))
        .unwrap_or_default()
}

pub fn parse_input(s: &str) -> HookInput {
    let Ok(v) = serde_json::from_str::<Value>(s) else {
        return HookInput::default();
    };
    let codex = std::env::var("ENGRAM_AGENT").is_ok_and(|a| a == "codex")
        || v.get("transcript_path")
            .and_then(Value::as_str)
            .is_some_and(|p| p.contains("/.codex/"));
    let mut files = vec![];
    if let Some(ti) = v.get("tool_input") {
        if let Some(fp) = ti.get("file_path").and_then(Value::as_str) {
            files.push(expand(fp));
        }
        patch_files(ti, &mut files);
    }
    HookInput { codex, files }
}

/// Paths from `*** Add|Update|Delete File: <path>` lines in any string of an apply_patch input
fn patch_files(v: &Value, files: &mut Vec<std::path::PathBuf>) {
    match v {
        Value::String(s) => {
            for l in s.lines() {
                for p in [
                    "*** Add File: ",
                    "*** Update File: ",
                    "*** Delete File: ",
                    "*** Move to: ",
                ] {
                    if let Some(f) = l.strip_prefix(p) {
                        files.push(expand(f.trim()));
                    }
                }
            }
        }
        Value::Array(a) => a.iter().for_each(|x| patch_files(x, files)),
        Value::Object(o) => o.values().for_each(|x| patch_files(x, files)),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_write() {
        let i = parse_input(r#"{"tool_input":{"file_path":"/m/a.md"}}"#);
        assert!(!i.codex);
        assert_eq!(i.files, vec![std::path::PathBuf::from("/m/a.md")]);
    }

    #[test]
    fn codex_apply_patch() {
        let i = parse_input(
            r#"{"transcript_path":"/u/.codex/sessions/x.jsonl","tool_input":{"command":"*** Begin Patch\n*** Add File: /m/b.md\n+x\n*** Update File: /m/c.md\n*** End Patch"}}"#,
        );
        assert!(i.codex);
        assert_eq!(i.files.len(), 2);
    }

    #[test]
    fn codex_by_transcript() {
        assert!(parse_input(r#"{"transcript_path":"/u/.codex/sessions/x.jsonl"}"#).codex);
        assert!(!parse_input(r#"{"transcript_path":"/u/.claude/projects/x.jsonl"}"#).codex);
    }
}
