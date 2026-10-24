//! Claude Code hook wiring: settings.json edits and the hook entry points.
use crate::config::{Config, claude_settings_path, expand};
use anyhow::{Context, Result};
use serde_json::{Map, Value, json};

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

/// Add engram hooks and `autoMemoryDirectory` to Claude Code's settings.json, keeping other hooks intact
pub fn install(cfg: &Config) -> Result<()> {
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
        list.push(hook_entry(
            &event.to_lowercase(),
            matcher,
            timeout,
            is_async,
        ));
    }
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

/// `tool_input.file_path` from the PostToolUse JSON on stdin
pub fn stdin_file_path() -> Option<std::path::PathBuf> {
    let mut s = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut s).ok()?;
    let v: Map<String, Value> = serde_json::from_str(&s).ok()?;
    let fp = v.get("tool_input")?.get("file_path")?.as_str()?;
    Some(expand(fp))
}
