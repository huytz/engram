use crate::backend::{Backend, Fetched};
use crate::config::{Collection, Config, expand};
use crate::crypto::{Keys, sha256_hex};
use crate::model::{Entry, Manifest, State, now_rfc3339, now_ts};
use crate::safety::{confined, valid_device, valid_key, valid_sha};
use anyhow::{Context, Result};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use futures_util::future::join_all;
use futures_util::stream::{self, StreamExt};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::time::Instant;

const TOMBSTONE_TTL: i64 = 90 * 86400;
const INDEX: &str = "MEMORY.md";
/// In-flight blob requests per device/collection; R2 speaks HTTP/1.1 so each is its own connection
const PARALLEL: usize = 16;
/// Raw bytes of new blobs a manifest may carry inline
const INLINE_MAX: usize = 256 * 1024;
/// Remote mtimes further ahead than this are clamped (seconds)
const MAX_SKEW: i64 = 86400;

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
    /// Security-relevant events of this pull, one line each (skipped devices, dropped keys, new devices)
    pub warnings: Vec<String>,
    /// Wall time of the network fetch for this collection
    pub ms: u128,
}

/// Outcome of one collection upload
pub struct Pushed<'c> {
    pub col: &'c Collection,
    pub changed: usize,
    pub blobs: usize,
    pub ms: u128,
}

pub struct Syncer<'a> {
    pub cfg: &'a Config,
    pub keys: &'a Keys,
    pub backend: &'a Backend,
    pub state: State,
    /// Reuse the device list from state while younger than `device_list_ttl`
    pub cached_list: bool,
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
    scan_since(col, None)
}

/// Like `scan`, but reuses the sha from `prev` when size+mtime match so unchanged files are not read
pub fn scan_since(col: &Collection, prev: Option<&Manifest>) -> Result<BTreeMap<String, Local>> {
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
        let md = e.metadata()?;
        let mtime = md
            .modified()?
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs() as i64;
        let cached = prev
            .and_then(|m| m.files.get(&rel))
            .filter(|p| p.size == md.len() && p.mtime == mtime)
            .and_then(|p| p.sha.clone());
        let sha = match cached {
            Some(sha) => sha,
            None => sha256_hex(&std::fs::read(e.path())?),
        };
        out.insert(
            rel,
            Local {
                sha,
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
    // Ends in `.tmp` so scans skip it; pid keeps concurrent writers apart
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy())
        .unwrap_or_default();
    let tmp = path.with_file_name(format!(".{name}.{}.engram.tmp", std::process::id()));
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

/// One device's manifest plus the blobs `apply` will need, fetched before any local write
struct Remote {
    dev: String,
    etag: String,
    manifest: Manifest,
    blobs: HashMap<String, Vec<u8>>,
    /// Manifest entries dropped by `sanitize`
    dropped: usize,
}

/// Result of fetching one collection, before anything is written
struct ColFetch {
    remotes: Vec<Remote>,
    listed: bool,
    warnings: Vec<String>,
    /// Our own manifest on the remote is missing or not the one we uploaded
    republish: bool,
}

/// Prepared upload of one collection: blobs to send, then the manifest that commits them
struct PushPlan<'c> {
    col: &'c Collection,
    uploads: Vec<(String, String)>,
    manifest: Manifest,
    local: BTreeMap<String, Local>,
    changed: usize,
}

impl<'a> Syncer<'a> {
    pub fn new(cfg: &'a Config, keys: &'a Keys, backend: &'a Backend) -> Result<Self> {
        Ok(Syncer {
            cfg,
            keys,
            backend,
            state: State::load()?,
            cached_list: false,
        })
    }

    /// Upload local changes of every collection concurrently; returns (collection, changed) per success
    pub async fn push_all<'c>(
        &mut self,
        cols: &[&'c Collection],
    ) -> (Vec<Pushed<'c>>, Option<anyhow::Error>) {
        let mut err = None;
        let mut plans = vec![];
        for c in cols {
            match self.plan_push(c) {
                Ok(Some(p)) => plans.push(p),
                Ok(None) => {}
                Err(e) => {
                    err.get_or_insert(e);
                }
            }
        }
        let this = &*self;
        let results = join_all(plans.iter().map(|p| async move {
            let t = Instant::now();
            (this.upload(p).await, t.elapsed().as_millis())
        }))
        .await;
        let mut done = vec![];
        for (plan, (r, ms)) in plans.into_iter().zip(results) {
            match r {
                Ok(etag) => {
                    self.state.own_etag.insert(plan.col.name.clone(), etag);
                    self.state.republish.remove(&plan.col.name);
                    done.push(Pushed {
                        col: plan.col,
                        changed: plan.changed,
                        blobs: plan.uploads.len(),
                        ms,
                    });
                    self.commit_push(plan);
                }
                Err(e) => {
                    err.get_or_insert(e);
                }
            }
        }
        if !done.is_empty()
            && let Err(e) = self.state.save()
        {
            err.get_or_insert(e);
        }
        (done, err)
    }

    /// Diff `col` against our last manifest; None when nothing changed
    fn plan_push<'c>(&self, col: &'c Collection) -> Result<Option<PushPlan<'c>>> {
        let dev = &self.cfg.device;
        let empty = Manifest::default();
        let own = self.state.own.get(&col.name).unwrap_or(&empty);
        // A missing root (unmounted, renamed) must not turn into tombstones for every file
        if !expand(&col.path).is_dir() {
            return Ok(None);
        }
        let local = scan_since(col, Some(own))?;
        // Blobs are content-addressed per device: a sha in our last manifest is already uploaded
        let mut have: HashSet<&str> = own
            .files
            .values()
            .filter_map(|e| e.sha.as_deref())
            .collect();
        let mut files: BTreeMap<String, Entry> = BTreeMap::new();
        let mut uploads = vec![];
        let mut changed = 0usize;
        let now = now_ts();

        for (rel, l) in &local {
            let prev = own.files.get(rel);
            if prev.and_then(|p| p.sha.as_deref()) != Some(l.sha.as_str()) {
                changed += 1;
                if have.insert(l.sha.as_str()) {
                    uploads.push((rel.clone(), l.sha.clone()));
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
        // Manifests from before the `collection` field are republished once so peers accept them
        if changed == 0
            && own.seq > 0
            && own.collection == col.name
            && !self.state.republish.contains(&col.name)
        {
            return Ok(None);
        }
        let root = expand(&col.path);
        let mut inline = BTreeMap::new();
        let mut budget = INLINE_MAX;
        for (rel, sha) in &uploads {
            let size = local[rel].size as usize;
            if size > budget {
                continue;
            }
            let data = std::fs::read(root.join(rel)).with_context(|| format!("read {rel}"))?;
            // Changed since scan: blob path; next push sends the new sha
            if sha256_hex(&data) != *sha {
                continue;
            }
            budget -= data.len();
            inline.insert(sha.clone(), B64.encode(&data));
        }
        let manifest = Manifest {
            device: dev.clone(),
            collection: col.name.clone(),
            // Peers reject seqs that go back; wall-clock floor survives a lost state.json
            seq: (own.seq + 1).max(chrono::Utc::now().timestamp_millis() as u64),
            updated_at: now_rfc3339(),
            files,
            inline,
        };
        Ok(Some(PushPlan {
            col,
            uploads,
            manifest,
            local,
            changed,
        }))
    }

    /// PUT non-inline blobs, then manifest ∥ inline blobs: 1 round trip if all fit
    async fn upload(&self, plan: &PushPlan<'_>) -> Result<String> {
        let (col, dev) = (&plan.col.name, &self.cfg.device);
        let root = expand(&plan.col.path);
        let put_blobs = |inlined: bool| {
            let root = &root;
            stream::iter(
                plan.uploads
                    .iter()
                    .filter(move |(_, sha)| plan.manifest.inline.contains_key(sha) == inlined)
                    .map(move |(rel, sha)| async move {
                        let data = match plan.manifest.inline.get(sha) {
                            Some(b) => B64.decode(b)?,
                            None => std::fs::read(root.join(rel))
                                .with_context(|| format!("read {rel}"))?,
                        };
                        self.backend
                            .put(&blob_key(col, dev, sha), self.keys.encrypt(&data)?)
                            .await
                            .map(|_| ())
                    }),
            )
            .buffer_unordered(PARALLEL)
            .collect::<Vec<Result<()>>>()
        };
        // Readers of the new manifest need these blobs before it lands
        for r in put_blobs(false).await {
            r?;
        }
        let body = self.keys.encrypt(&serde_json::to_vec(&plan.manifest)?)?;
        let key = manifest_key(col, dev);
        let (inlined, manifest) = tokio::join!(put_blobs(true), self.backend.put(&key, body));
        for r in inlined {
            r?;
        }
        manifest
    }

    fn commit_push(&mut self, plan: PushPlan<'_>) {
        let name = &plan.col.name;
        let applied = self.state.applied.entry(name.clone()).or_default();
        for (rel, l) in &plan.local {
            applied.insert(rel.clone(), l.sha.clone());
        }
        applied.retain(|k, _| plan.local.contains_key(k));
        let mut manifest = plan.manifest;
        manifest.inline.clear();
        self.state.own.insert(name.clone(), manifest);
    }

    /// Fetch every collection concurrently, then merge sequentially; one result per collection
    pub async fn pull_all<'c>(
        &mut self,
        cols: &[&'c Collection],
    ) -> Vec<(&'c Collection, Result<Report>)> {
        let this = &*self;
        let fetched = join_all(cols.iter().map(|c| async move {
            let t = Instant::now();
            (this.fetch_col(c).await, t.elapsed().as_millis())
        }))
        .await;
        let mut out = vec![];
        let mut dirty = false;
        for (col, (f, ms)) in cols.iter().zip(fetched) {
            let r = f.and_then(|cf| {
                if cf.listed {
                    self.state.listed_at.insert(col.name.clone(), now_ts());
                    dirty = true;
                }
                if cf.republish {
                    dirty |= self.state.republish.insert(col.name.clone());
                }
                dirty |= !cf.remotes.is_empty();
                self.apply_col(col, cf.remotes).map(|mut r| {
                    r.warnings = cf.warnings;
                    r
                })
            });
            let r = r.map(|mut r| {
                r.ms = ms;
                r
            });
            out.push((*col, r));
        }
        if dirty && let Err(e) = self.state.save() {
            out.push((cols[0], Err(e)));
        }
        out
    }

    fn apply_col(&mut self, col: &Collection, remotes: Vec<Remote>) -> Result<Report> {
        let root = expand(&col.path);
        let mut report = Report {
            changes: vec![],
            new_index_lines: vec![],
            warnings: vec![],
            ms: 0,
        };
        if remotes.is_empty() {
            return Ok(report);
        }
        let index_before = std::fs::read_to_string(root.join(INDEX)).unwrap_or_default();
        for r in remotes {
            self.apply(col, &root, &r, &mut report)?;
            self.state
                .devices
                .entry(col.name.clone())
                .or_default()
                .insert(
                    r.dev.clone(),
                    crate::model::DevState {
                        etag: r.etag,
                        seq: r.manifest.seq,
                    },
                );
        }
        let index_after = std::fs::read_to_string(root.join(INDEX)).unwrap_or_default();
        report.new_index_lines = crate::memorymd::new_lines(&index_before, &index_after);
        Ok(report)
    }

    /// LIST ∥ known-device GETs, LIST skipped if cache fresh; returns (remotes, listed, skipped devices)
    async fn fetch_col(&self, col: &Collection) -> Result<ColFetch> {
        let me = self.cfg.device.as_str();
        let ignored = |d: &str| self.cfg.ignored_devices.iter().any(|i| i == d);
        let known: Vec<String> = self
            .state
            .devices
            .get(&col.name)
            .into_iter()
            .flat_map(|m| m.keys().cloned())
            .filter(|d| d != me && valid_device(d) && !ignored(d))
            .collect();
        let fresh_list = self.cached_list
            && !known.is_empty()
            && self
                .state
                .listed_at
                .get(&col.name)
                .is_some_and(|t| now_ts().saturating_sub(*t) < self.cfg.device_list_ttl as i64);
        let prefix = format!("{}/", col.name);
        let list = async {
            match fresh_list {
                true => Ok(None),
                false => self.backend.list_children(&prefix).await.map(Some),
            }
        };
        // Nobody else should write under our name: a changed or missing own manifest means impersonation or loss
        let own_etag = self.state.own_etag.get(&col.name).cloned();
        let self_check = async {
            match &own_etag {
                Some(et) => self
                    .backend
                    .get(&manifest_key(&col.name, me), Some(et))
                    .await
                    .ok(),
                None => None,
            }
        };
        let (listed, known_res, own) = tokio::join!(
            list,
            join_all(known.iter().map(|d| self.fetch_dev(col, d))),
            self_check
        );
        let listed = listed?;
        let did_list = listed.is_some();
        let mut fresh: Vec<String> = listed
            .unwrap_or_default()
            .into_iter()
            .filter(|d| d != me && !known.contains(d) && valid_device(d) && !ignored(d))
            .collect();
        fresh.sort();
        fresh.dedup();
        let fresh_res = join_all(fresh.iter().map(|d| self.fetch_dev(col, d))).await;
        // One bad device must not block the others
        let (mut out, mut warnings, mut failed) = (vec![], vec![], vec![]);
        for (i, (d, r)) in known
            .iter()
            .chain(&fresh)
            .zip(known_res.into_iter().chain(fresh_res))
            .enumerate()
        {
            match r {
                Ok(Some(r)) => {
                    if i >= known.len() {
                        warnings.push(format!(
                            "new device {d} (check `engram status` if unexpected)"
                        ));
                    }
                    if r.dropped > 0 {
                        warnings.push(format!("{d}: ignored {} unsafe file key(s)", r.dropped));
                    }
                    out.push(r);
                }
                Ok(None) => {}
                Err(e) => failed.push(format!("skipped {d}: {e:#}")),
            }
        }
        // Every peer rejected (wrong identity, bad config): an error, not a warning
        if !failed.is_empty() && failed.len() == known.len() + fresh.len() {
            anyhow::bail!("{}", failed.join("; "));
        }
        warnings.extend(failed);
        let republish = match own {
            Some(Fetched::Found { .. }) => {
                warnings.push(format!(
                    "own manifest {}/{me} was overwritten by another writer; republishing",
                    col.name
                ));
                true
            }
            Some(Fetched::Missing) => {
                warnings.push(format!(
                    "own manifest {}/{me} is missing; republishing",
                    col.name
                ));
                true
            }
            _ => false,
        };
        out.sort_by(|a, b| a.dev.cmp(&b.dev));
        Ok(ColFetch {
            remotes: out,
            listed: did_list,
            warnings,
            republish,
        })
    }

    /// Conditional GET of a device manifest plus a parallel fetch of the blobs it will need
    async fn fetch_dev(&self, col: &Collection, dev: &str) -> Result<Option<Remote>> {
        let etag = self
            .state
            .devices
            .get(&col.name)
            .and_then(|m| m.get(dev))
            .map(|d| d.etag.as_str())
            .filter(|e| !e.is_empty());
        let (data, etag) = match self
            .backend
            .get(&manifest_key(&col.name, dev), etag)
            .await?
        {
            Fetched::NotModified | Fetched::Missing => return Ok(None),
            Fetched::Found { data, etag } => (data, etag),
        };
        let mut manifest: Manifest =
            serde_json::from_slice(&self.keys.decrypt(&data)?).context("manifest json")?;
        anyhow::ensure!(
            !manifest.collection.is_empty(),
            "{dev} runs engram < 0.1.7; upgrade it so its manifests can be checked"
        );
        // A manifest copied to another prefix or replayed from the past is not this device's latest
        anyhow::ensure!(
            manifest.device == dev && manifest.collection == col.name,
            "manifest is for {}/{}, not {}/{dev}",
            manifest.collection,
            manifest.device,
            col.name
        );
        let last = self
            .state
            .devices
            .get(&col.name)
            .and_then(|m| m.get(dev))
            .map_or(0, |d| d.seq);
        anyhow::ensure!(
            manifest.seq > last,
            "manifest seq {} not newer than {last} (replayed?)",
            manifest.seq
        );
        let dropped = sanitize(&mut manifest);
        let root = expand(&col.path);
        let mut need: Vec<String> = manifest
            .files
            .iter()
            .filter(|(rel, e)| !excluded(rel, col) && self.unseen(&col.name, dev, rel, e))
            .filter_map(|(rel, e)| {
                let sha = e.sha.as_deref()?;
                let same = rel.as_str() != INDEX
                    && std::fs::read(root.join(rel))
                        .ok()
                        .map(|d| sha256_hex(&d))
                        .as_deref()
                        == Some(sha);
                (!same).then(|| sha.to_string())
            })
            .collect();
        need.sort_unstable();
        need.dedup();
        let mut blobs = HashMap::new();
        need.retain(|sha| {
            let hit = manifest
                .inline
                .get(sha)
                .and_then(|b| B64.decode(b).ok())
                .filter(|d| sha256_hex(d) == *sha);
            match hit {
                Some(d) => {
                    blobs.insert(sha.clone(), d);
                    false
                }
                None => true,
            }
        });
        let mut gets = stream::iter(need.into_iter().map(|sha| async move {
            match self
                .backend
                .get(&blob_key(&col.name, dev, &sha), None)
                .await?
            {
                Fetched::Found { data, .. } => {
                    let plain = self.keys.decrypt(&data)?;
                    anyhow::ensure!(
                        sha256_hex(&plain) == sha,
                        "blob {sha} content mismatch on {dev}"
                    );
                    Ok((sha.clone(), plain))
                }
                _ => anyhow::bail!("blob {sha} missing on {dev}"),
            }
        }))
        .buffer_unordered(PARALLEL);
        while let Some(r) = gets.next().await {
            let (sha, data) = r?;
            blobs.insert(sha, data);
        }
        Ok(Some(Remote {
            dev: dev.to_string(),
            etag,
            manifest,
            blobs,
            dropped,
        }))
    }

    fn unseen(&self, col: &str, dev: &str, rel: &str, e: &Entry) -> bool {
        let seen = self
            .state
            .seen
            .get(col)
            .and_then(|d| d.get(dev))
            .and_then(|p| p.get(rel));
        seen.map(String::as_str) != Some(e.key().as_str())
    }

    fn apply(
        &mut self,
        col: &Collection,
        root: &Path,
        r: &Remote,
        report: &mut Report,
    ) -> Result<()> {
        let (dev, m) = (r.dev.as_str(), &r.manifest);
        let blob = |sha: &str| -> Result<&Vec<u8>> {
            r.blobs
                .get(sha)
                .ok_or_else(|| anyhow::anyhow!("blob {sha} missing on {dev}"))
        };
        let cname = col.name.clone();
        let mut index_remote: Option<(Entry, String)> = None;
        let mut pulled_files: HashSet<String> = HashSet::new();

        for (rel, e) in &m.files {
            if excluded(rel, col) || !self.unseen(&cname, dev, rel, e) {
                continue;
            }
            let ekey = e.key();
            if rel == INDEX {
                index_remote = Some((e.clone(), ekey));
                continue;
            }
            // Symlinked parent inside the root could redirect the write outside it
            let Some(path) = confined(root, rel) else {
                continue;
            };
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
                    let data = blob(rsha)?;
                    if unchanged {
                        write_file(&path, data, e.mtime)?;
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
                        let loser = free_conflict_name(
                            root,
                            rel,
                            if remote_wins { &self.cfg.device } else { dev },
                        );
                        if remote_wins {
                            std::fs::rename(&path, root.join(&loser))?;
                            write_file(&path, data, e.mtime)?;
                            pulled_files.insert(rel.clone());
                        } else {
                            write_file(&root.join(&loser), data, e.mtime)?;
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
                let remote = String::from_utf8_lossy(blob(rsha)?).to_string();
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

/// Drop keys that are not plain relative paths, bad shas, case-insensitive duplicates; clamp mtimes
fn sanitize(m: &mut Manifest) -> usize {
    let now = now_ts();
    let before = m.files.len();
    let mut folded = HashSet::new();
    m.files.retain(|k, e| {
        valid_key(k) && e.sha.as_deref().is_none_or(valid_sha) && folded.insert(k.to_lowercase())
    });
    for e in m.files.values_mut() {
        e.mtime = e.mtime.clamp(0, now + MAX_SKEW);
    }
    m.inline.retain(|k, _| valid_sha(k));
    before - m.files.len()
}

/// First conflict name not on disk yet, so a later conflict never replaces an earlier copy
fn free_conflict_name(root: &Path, rel: &str, loser_dev: &str) -> String {
    let base = conflict_name(rel, loser_dev);
    if !root.join(&base).exists() {
        return base;
    }
    (2..)
        .map(|n| conflict_name(rel, &format!("{loser_dev}-{n}")))
        .find(|c| !root.join(c).exists())
        .unwrap_or(base)
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
