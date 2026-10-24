use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Entry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha: Option<String>,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub mtime: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<i64>,
}

impl Entry {
    /// Identity of this entry for change detection
    pub fn key(&self) -> String {
        match (&self.sha, self.deleted_at) {
            (Some(s), _) => s.clone(),
            (None, Some(t)) => format!("deleted:{t}"),
            _ => "empty".into(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Manifest {
    pub device: String,
    pub seq: u64,
    pub updated_at: String,
    pub files: BTreeMap<String, Entry>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DevState {
    pub etag: String,
    pub seq: u64,
}

/// Local bookkeeping: what we last wrote to disk and what we have seen from each device
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct State {
    /// collection -> device -> last manifest seen
    #[serde(default)]
    pub devices: BTreeMap<String, BTreeMap<String, DevState>>,
    /// collection -> path -> sha of content last applied by sync
    #[serde(default)]
    pub applied: BTreeMap<String, BTreeMap<String, String>>,
    /// collection -> device -> path -> entry key already processed
    #[serde(default)]
    pub seen: BTreeMap<String, BTreeMap<String, BTreeMap<String, String>>>,
    /// collection -> our own last pushed manifest
    #[serde(default)]
    pub own: BTreeMap<String, Manifest>,
    /// sha of the plugin list last published
    #[serde(default)]
    pub plugins_sha: Option<String>,
}

impl State {
    fn path() -> std::path::PathBuf {
        crate::config::home().join("state.json")
    }

    pub fn load() -> Result<State> {
        match std::fs::read_to_string(Self::path()) {
            Ok(s) => Ok(serde_json::from_str(&s)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn save(&self) -> Result<()> {
        crate::config::write_private(&Self::path(), serde_json::to_string(self)?.as_bytes())
    }
}

pub fn now_ts() -> i64 {
    chrono::Utc::now().timestamp()
}

pub fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}
