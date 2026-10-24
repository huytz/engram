use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Collection {
    pub name: String,
    pub path: String,
    #[serde(default)]
    pub exclude: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub device: String,
    /// `s3://bucket` or `file:///abs/dir`
    pub remote: String,
    #[serde(default)]
    pub endpoint: Option<String>,
    #[serde(default)]
    pub region: Option<String>,
    #[serde(default)]
    pub access_key_id: Option<String>,
    #[serde(default)]
    pub secret_access_key: Option<String>,
    pub collections: Vec<Collection>,
}

/// `$ENGRAM_HOME` or `~/.engram`
pub fn home() -> PathBuf {
    if let Ok(h) = std::env::var("ENGRAM_HOME") {
        return PathBuf::from(h);
    }
    dirs::home_dir().expect("no home dir").join(".engram")
}

pub fn expand(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        return dirs::home_dir().expect("no home dir").join(rest);
    }
    PathBuf::from(p)
}

/// Settings file Claude Code reads: `$CLAUDE_CONFIG_DIR/settings.json` or `~/.claude/settings.json`
pub fn claude_settings_path() -> PathBuf {
    let dir = std::env::var("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| dirs::home_dir().expect("no home dir").join(".claude"));
    dir.join("settings.json")
}

impl Config {
    pub fn path() -> PathBuf {
        home().join("config.json")
    }

    pub fn load() -> Result<Config> {
        let p = Self::path();
        let s = std::fs::read_to_string(&p)
            .with_context(|| format!("read {} (run `engram init` first)", p.display()))?;
        Ok(serde_json::from_str(&s)?)
    }

    pub fn save(&self) -> Result<()> {
        std::fs::create_dir_all(home())?;
        write_private(
            &Self::path(),
            serde_json::to_string_pretty(self)?.as_bytes(),
        )
    }

    pub fn collection(&self, name: &str) -> Option<&Collection> {
        self.collections.iter().find(|c| c.name == name)
    }

    /// Collection whose directory contains `file`
    pub fn collection_for(&self, file: &Path) -> Option<&Collection> {
        let file = file.canonicalize().unwrap_or_else(|_| file.to_path_buf());
        self.collections.iter().find(|c| {
            let root = expand(&c.path);
            let root = root.canonicalize().unwrap_or(root);
            file.starts_with(&root)
        })
    }
}

pub fn write_private(path: &Path, data: &[u8]) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        std::io::Write::write_all(&mut f, data)?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}
