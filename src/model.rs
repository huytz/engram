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
    /// Collection this manifest belongs to; peers reject a manifest copied under another prefix
    #[serde(default)]
    pub collection: String,
    pub seq: u64,
    pub updated_at: String,
    pub files: BTreeMap<String, Entry>,
    /// sha -> base64 of blobs new in this push; readers skip their GET
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub inline: BTreeMap<String, String>,
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
    /// collection -> unix time of the last device LIST
    #[serde(default)]
    pub listed_at: BTreeMap<String, i64>,
    /// collection -> ETag of our own manifest as we last uploaded it (self-check on pull)
    #[serde(default)]
    pub own_etag: BTreeMap<String, String>,
    /// Collections whose own manifest must be uploaded again (missing or overwritten remotely)
    #[serde(default)]
    pub republish: std::collections::BTreeSet<String>,
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

    /// Drop all sync bookkeeping for one collection
    pub fn forget(&mut self, col: &str) {
        self.devices.remove(col);
        self.applied.remove(col);
        self.seen.remove(col);
        self.own.remove(col);
        self.listed_at.remove(col);
        self.own_etag.remove(col);
        self.republish.remove(col);
    }

    /// Drop what we know about one peer device
    pub fn forget_device(&mut self, dev: &str) {
        for m in self.devices.values_mut() {
            m.remove(dev);
        }
        for m in self.seen.values_mut() {
            m.remove(dev);
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
