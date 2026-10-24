use crate::backend::{Backend, Fetched};
use crate::config::{Collection, Config, expand};
use crate::crypto::{Keys, sha256_hex};
use crate::model::{Entry, Manifest, State, now_rfc3339, now_ts};
use anyhow::{Context, Result};
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

const TOMBSTONE_TTL: i64 = 90 * 86400;
const INDEX: &str = "MEMORY.md";

pub struct Local {
    pub sha: String,
    pub size: u64,
    pub mtime: i64,
}

#[derive(Debug)]
pub struct Change {
    pub device: String,
    pub path: String,
    pub kind: &'static str,
}

pub struct Report {
    pub changes: Vec<Change>,
    pub new_index_lines: Vec<String>,
}

pub struct Syncer<'a> {
    pub cfg: &'a Config,
    pub keys: &'a Keys,
    pub backend: &'a Backend,
    pub state: State,
}

/// Conflict copies stay local so one device's mess never spreads to the others
fn excluded(rel: &str, col: &Collection) -> bool {
    if rel.ends_with(".DS_Store") || rel.ends_with(".tmp") || rel.contains(".conflict-") {
        return true;
    }
    col.exclude.iter().any(|x| {
        let x = x.trim_end_matches('/');
        rel == x || rel.starts_with(&format!("{x}/"))
    })
}

pub fn scan(col: &Collection) -> Result<BTreeMap<String, Local>> {
    let root = expand(&col.path);
    let mut out = BTreeMap::new();
    if !root.exists() {
        return Ok(out);
    }
    for e in walkdir::WalkDir::new(&root)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if !e.file_type().is_file() {
            continue;
        }
        let rel = e
            .path()
            .strip_prefix(&root)?
            .to_string_lossy()
            .replace('\\', "/");
        if excluded(&rel, col) {
            continue;
        }
        let data = std::fs::read(e.path())?;
        let md = e.metadata()?;
        let mtime = md
            .modified()?
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs() as i64;
        out.insert(
            rel,
            Local {
                sha: sha256_hex(&data),
                size: md.len(),
                mtime,
            },
        );
    }
    Ok(out)
}

fn manifest_key(col: &str, dev: &str) -> String {
    format!("{col}/{dev}/manifest.json")
}

fn blob_key(col: &str, dev: &str, sha: &str) -> String {
    format!("{col}/{dev}/blobs/{sha}")
}

fn write_file(path: &Path, data: &[u8], mtime: i64) -> Result<()> {
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p)?;
    }
    let tmp = path.with_extension("engram-tmp");
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path)?;
    if mtime > 0 {
        let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(mtime as u64);
        let _ = std::fs::File::options()
            .write(true)
            .open(path)
            .and_then(|f| f.set_modified(t));
    }
    Ok(())
}

impl<'a> Syncer<'a> {
    pub fn new(cfg: &'a Config, keys: &'a Keys, backend: &'a Backend) -> Result<Self> {
        Ok(Syncer {
            cfg,
            keys,
            backend,
            state: State::load()?,
        })
    }

    /// Upload local changes of `col` under our device prefix; returns number of changed entries
    pub async fn push(&mut self, col: &Collection) -> Result<usize> {
        let dev = &self.cfg.device;
        let local = scan(col)?;
        let own = self.state.own.get(&col.name).cloned().unwrap_or_default();
        let mut files: BTreeMap<String, Entry> = BTreeMap::new();
        let mut changed = 0usize;
        let now = now_ts();

        for (rel, l) in &local {
            let prev = own.files.get(rel);
            if prev.and_then(|p| p.sha.as_deref()) != Some(l.sha.as_str()) {
                changed += 1;
                let key = blob_key(&col.name, dev, &l.sha);
                if !self.backend.exists(&key).await? {
                    let data = std::fs::read(expand(&col.path).join(rel))?;
                    self.backend.put(&key, self.keys.encrypt(&data)?).await?;
                }
            }
            files.insert(
                rel.clone(),
                Entry {
                    sha: Some(l.sha.clone()),
                    size: l.size,
                    mtime: l.mtime,
                    deleted_at: None,
                },
            );
        }
        for (rel, e) in &own.files {
            if local.contains_key(rel) {
                continue;
            }
            match e.deleted_at {
                Some(t) if now - t > TOMBSTONE_TTL => {}
                Some(t) => {
                    files.insert(
                        rel.clone(),
                        Entry {
                            deleted_at: Some(t),
                            ..Default::default()
                        },
                    );
                }
                None => {
                    changed += 1;
                    files.insert(
                        rel.clone(),
                        Entry {
                            deleted_at: Some(now),
                            ..Default::default()
                        },
                    );
                }
            }
        }
        if changed == 0 && own.seq > 0 {
            return Ok(0);
        }
        let m = Manifest {
            device: dev.clone(),
            seq: own.seq + 1,
            updated_at: now_rfc3339(),
            files,
        };
        let body = self.keys.encrypt(serde_json::to_vec(&m)?.as_slice())?;
        self.backend
            .put(&manifest_key(&col.name, dev), body)
            .await?;
        let applied = self.state.applied.entry(col.name.clone()).or_default();
        for (rel, l) in &local {
            applied.insert(rel.clone(), l.sha.clone());
        }
        applied.retain(|k, _| local.contains_key(k));
        self.state.own.insert(col.name.clone(), m);
        self.state.save()?;
        Ok(changed)
    }

    /// Merge other devices' manifests into the local directory of `col`
    pub async fn pull(&mut self, col: &Collection) -> Result<Report> {
        let root = expand(&col.path);
        let mut report = Report {
            changes: vec![],
            new_index_lines: vec![],
        };
        let index_before = std::fs::read_to_string(root.join(INDEX)).unwrap_or_default();

        let keys = self.backend.list(&format!("{}/", col.name)).await?;
        let mut devices: Vec<String> = keys
            .iter()
            .filter_map(|k| {
                let parts: Vec<&str> = k.split('/').collect();
                (parts.len() == 3 && parts[2] == "manifest.json" && parts[1] != self.cfg.device)
                    .then(|| parts[1].to_string())
            })
            .collect();
        devices.sort();
        devices.dedup();

        for dev in devices {
            let ds = self
                .state
                .devices
                .get(&col.name)
                .and_then(|m| m.get(&dev))
                .cloned()
                .unwrap_or_default();
            let etag = (!ds.etag.is_empty()).then_some(ds.etag.as_str());
            let (data, new_etag) = match self
                .backend
                .get(&manifest_key(&col.name, &dev), etag)
                .await?
            {
                Fetched::NotModified | Fetched::Missing => continue,
                Fetched::Found { data, etag } => (data, etag),
            };
            let m: Manifest =
                serde_json::from_slice(&self.keys.decrypt(&data)?).context("manifest json")?;
            self.apply(col, &root, &dev, &m, &mut report).await?;
            self.state
                .devices
                .entry(col.name.clone())
                .or_default()
                .insert(
                    dev.clone(),
                    crate::model::DevState {
                        etag: new_etag,
                        seq: m.seq,
                    },
                );
            self.state.save()?;
        }

        let index_after = std::fs::read_to_string(root.join(INDEX)).unwrap_or_default();
        report.new_index_lines = crate::memorymd::new_lines(&index_before, &index_after);
        Ok(report)
    }

    async fn fetch_blob(&self, col: &str, dev: &str, sha: &str) -> Result<Vec<u8>> {
        match self.backend.get(&blob_key(col, dev, sha), None).await? {
            Fetched::Found { data, .. } => self.keys.decrypt(&data),
            _ => anyhow::bail!("blob {sha} missing on {dev}"),
        }
    }

    async fn apply(
        &mut self,
        col: &Collection,
        root: &Path,
        dev: &str,
        m: &Manifest,
        report: &mut Report,
    ) -> Result<()> {
        let cname = col.name.clone();
        let mut index_remote: Option<(Entry, String)> = None;
        let mut pulled_files: HashSet<String> = HashSet::new();

        for (rel, e) in &m.files {
            if excluded(rel, col) {
                continue;
            }
            let ekey = e.key();
            let seen = self
                .state
                .seen
                .get(&cname)
                .and_then(|d| d.get(dev))
                .and_then(|p| p.get(rel))
                .cloned();
            if seen.as_deref() == Some(ekey.as_str()) {
                continue;
            }
            if rel == INDEX {
                index_remote = Some((e.clone(), ekey));
                continue;
            }
            let path = root.join(rel);
            let local_data = std::fs::read(&path).ok();
            let local_sha = local_data.as_deref().map(sha256_hex);
            let applied = self
                .state
                .applied
                .get(&cname)
                .and_then(|a| a.get(rel))
                .cloned();
            let unchanged = local_sha.is_none() || local_sha == applied;

            match &e.sha {
                None => {
                    if local_sha.is_some() && unchanged {
                        std::fs::remove_file(&path)?;
                        self.state
                            .applied
                            .entry(cname.clone())
                            .or_default()
                            .remove(rel);
                        report.changes.push(Change {
                            device: dev.into(),
                            path: rel.clone(),
                            kind: "deleted",
                        });
                    }
                }
                Some(rsha) if Some(rsha) == local_sha.as_ref() => {
                    self.state
                        .applied
                        .entry(cname.clone())
                        .or_default()
                        .insert(rel.clone(), rsha.clone());
                }
                Some(rsha) => {
                    let data = self.fetch_blob(&cname, dev, rsha).await?;
                    if unchanged {
                        write_file(&path, &data, e.mtime)?;
                        self.state
                            .applied
                            .entry(cname.clone())
                            .or_default()
                            .insert(rel.clone(), rsha.clone());
                        report.changes.push(Change {
                            device: dev.into(),
                            path: rel.clone(),
                            kind: if local_sha.is_some() {
                                "updated"
                            } else {
                                "added"
                            },
                        });
                        pulled_files.insert(rel.clone());
                    } else {
                        // Both sides changed: newer mtime wins, loser kept beside it
                        let local_mtime = std::fs::metadata(&path)?
                            .modified()?
                            .duration_since(std::time::UNIX_EPOCH)?
                            .as_secs() as i64;
                        let remote_wins = e.mtime > local_mtime
                            || (e.mtime == local_mtime && dev > self.cfg.device.as_str());
                        let loser =
                            conflict_name(rel, if remote_wins { &self.cfg.device } else { dev });
                        if remote_wins {
                            std::fs::rename(&path, root.join(&loser))?;
                            write_file(&path, &data, e.mtime)?;
                            pulled_files.insert(rel.clone());
                        } else {
                            write_file(&root.join(&loser), &data, e.mtime)?;
                        }
                        let now_sha = sha256_hex(&std::fs::read(&path)?);
                        self.state
                            .applied
                            .entry(cname.clone())
                            .or_default()
                            .insert(rel.clone(), now_sha);
                        report.changes.push(Change {
                            device: dev.into(),
                            path: format!("{rel} (conflict → {loser})"),
                            kind: "conflict",
                        });
                    }
                }
            }
            self.state
                .seen
                .entry(cname.clone())
                .or_default()
                .entry(dev.into())
                .or_default()
                .insert(rel.clone(), ekey);
        }

        if let Some((e, ekey)) = index_remote {
            if let Some(rsha) = &e.sha {
                let path = root.join(INDEX);
                let remote =
                    String::from_utf8_lossy(&self.fetch_blob(&cname, dev, rsha).await?).to_string();
                let local = std::fs::read_to_string(&path).unwrap_or_default();
                let applied = self
                    .state
                    .applied
                    .get(&cname)
                    .and_then(|a| a.get(INDEX))
                    .cloned();
                let local_unchanged = Some(sha256_hex(local.as_bytes())) == applied;
                // No local edits since last sync → remote is strictly newer, adopt it verbatim (propagates removals)
                let merged = if local.trim().is_empty() || local_unchanged {
                    remote
                } else {
                    crate::memorymd::merge(&local, &remote, &pulled_files)
                };
                if merged != local {
                    write_file(&path, merged.as_bytes(), 0)?;
                    report.changes.push(Change {
                        device: dev.into(),
                        path: INDEX.into(),
                        kind: "merged",
                    });
                }
                self.state
                    .applied
                    .entry(cname.clone())
                    .or_default()
                    .insert(INDEX.into(), sha256_hex(merged.as_bytes()));
            }
            self.state
                .seen
                .entry(cname.clone())
                .or_default()
                .entry(dev.into())
                .or_default()
                .insert(INDEX.into(), ekey);
        }
        Ok(())
    }
}

/// `a/b.md` → `a/b.conflict-<dev>.md`; keeps the extension so Claude still reads it as markdown
fn conflict_name(rel: &str, loser_dev: &str) -> String {
    let (dir, file) = rel
        .rsplit_once('/')
        .map(|(d, f)| (format!("{d}/"), f))
        .unwrap_or((String::new(), rel));
    match file.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => format!("{dir}{stem}.conflict-{loser_dev}.{ext}"),
        _ => format!("{dir}{file}.conflict-{loser_dev}"),
    }
}
