//! Plugin inventory: publish this device's installed Claude Code plugins and diff against others.
use crate::backend::{Backend, Fetched};
use crate::crypto::{Keys, sha256_hex};
use anyhow::Result;
use serde_json::Value;
use std::collections::BTreeMap;

/// `name@marketplace -> version` from Claude Code's installed_plugins.json
pub fn local_list() -> Result<BTreeMap<String, String>> {
    let dir = std::env::var("CLAUDE_CONFIG_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| dirs::home_dir().expect("no home dir").join(".claude"));
    let p = dir.join("plugins/installed_plugins.json");
    let mut out = BTreeMap::new();
    let Ok(s) = std::fs::read_to_string(&p) else {
        return Ok(out);
    };
    let v: Value = serde_json::from_str(&s)?;
    let plugins = v.get("plugins").and_then(|p| p.as_object());
    for (name, installs) in plugins.into_iter().flatten() {
        let version = installs
            .as_array()
            .and_then(|a| a.first())
            .and_then(|i| i.get("version"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        out.insert(name.clone(), version.to_string());
    }
    Ok(out)
}

fn key(dev: &str) -> String {
    format!("plugins/{dev}/list.json")
}

/// Upload the local list if it changed since the last push; returns true when uploaded
pub async fn publish(
    be: &Backend,
    keys: &Keys,
    device: &str,
    last_sha: Option<&str>,
) -> Result<Option<String>> {
    let body = serde_json::to_vec(&local_list()?)?;
    let sha = sha256_hex(&body);
    if last_sha == Some(sha.as_str()) {
        return Ok(None);
    }
    be.put(&key(device), keys.encrypt(&body)?).await?;
    Ok(Some(sha))
}

/// Other devices' lists
pub async fn fetch_all(
    be: &Backend,
    keys: &Keys,
    device: &str,
) -> Result<BTreeMap<String, BTreeMap<String, String>>> {
    let mut out = BTreeMap::new();
    for k in be.list("plugins/").await? {
        let parts: Vec<&str> = k.split('/').collect();
        if parts.len() != 3 || parts[2] != "list.json" || parts[1] == device {
            continue;
        }
        if let Fetched::Found { data, .. } = be.get(&k, None).await? {
            let list: BTreeMap<String, String> = serde_json::from_slice(&keys.decrypt(&data)?)?;
            out.insert(parts[1].to_string(), list);
        }
    }
    Ok(out)
}

/// Print what each side is missing, with the install commands to fix it
pub fn print_diff(
    device: &str,
    mine: &BTreeMap<String, String>,
    others: &BTreeMap<String, BTreeMap<String, String>>,
) {
    if others.is_empty() {
        println!("no other device has published a plugin list yet");
        return;
    }
    for (dev, theirs) in others {
        let missing_here: Vec<&String> = theirs.keys().filter(|k| !mine.contains_key(*k)).collect();
        let missing_there: Vec<&String> =
            mine.keys().filter(|k| !theirs.contains_key(*k)).collect();
        println!(
            "{device} vs {dev}: {} here, {} there",
            mine.len(),
            theirs.len()
        );
        if missing_here.is_empty() && missing_there.is_empty() {
            println!("  in sync");
            continue;
        }
        if !missing_here.is_empty() {
            println!("  installed on {dev} but not here:");
            for p in missing_here {
                println!("    claude plugin install {p}");
            }
        }
        if !missing_there.is_empty() {
            println!("  installed here but not on {dev}:");
            for p in missing_there {
                println!("    {p}");
            }
        }
    }
}
