mod backend;
mod config;
mod crypto;
mod hooks;
mod memorymd;
mod model;
mod plugins;
mod safety;
mod sync;

use anyhow::{Context, Result, bail};
use backend::Backend;
use clap::{Parser, Subcommand};
use config::{Collection, Config, expand, home};
use crypto::Keys;
use std::io::Write;
use sync::Syncer;

#[derive(Parser)]
#[command(
    name = "engram",
    version,
    about = "Unified Claude Code memory sync via R2/S3"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create config + identity, import existing memory, push once
    Init {
        /// Device name used as the remote prefix (default: `hostname -s`)
        #[arg(long)]
        device: Option<String>,
        /// s3://bucket or file:///abs/dir
        #[arg(long, env = "ENGRAM_REMOTE")]
        remote: String,
        #[arg(long, env = "ENGRAM_ENDPOINT")]
        endpoint: Option<String>,
        #[arg(long, env = "ENGRAM_ACCESS_KEY", hide_env_values = true)]
        access_key: Option<String>,
        #[arg(long, env = "ENGRAM_SECRET_KEY", hide_env_values = true)]
        secret_key: Option<String>,
        /// Memory dir to copy into ~/.engram/memory (default: Claude's current project memory)
        #[arg(long)]
        import: Option<String>,
        /// Skip syncing ~/.claude/skills
        #[arg(long)]
        no_skills: bool,
        /// Hooks come from the engram plugin: also set autoMemoryDirectory (= install-hooks --plugin)
        #[arg(long)]
        plugin: bool,
    },
    /// Fetch and merge other devices' changes
    Pull {
        #[arg(long)]
        collection: Option<String>,
    },
    /// Upload local changes
    Push {
        #[arg(long)]
        collection: Option<String>,
        /// Only push the collection containing this file
        #[arg(long)]
        if_path: Option<String>,
    },
    /// Pull then push
    Sync,
    /// Show devices, seqs and local pending changes
    Status,
    /// Wire hooks + autoMemoryDirectory into Claude Code settings.json
    InstallHooks {
        /// Hooks come from the engram plugin: set autoMemoryDirectory, remove engram hooks
        #[arg(long)]
        plugin: bool,
    },
    UninstallHooks,
    /// Show or hide per-hook timings (`engram pull 412ms · ...`) in the agent UI
    HookTiming {
        #[arg(value_parser = ["on", "off"])]
        state: String,
    },
    /// Entry points called by Claude Code hooks (fail soft, log to engram.log)
    Hook {
        event: String,
    },
    /// Manage devices on the remote
    Device {
        #[command(subcommand)]
        cmd: DeviceCmd,
    },
    /// Manage synced directories
    Collection {
        #[command(subcommand)]
        cmd: CollectionCmd,
    },
    /// Compare installed Claude Code plugins with other devices
    Plugins,
}

#[derive(Subcommand)]
enum DeviceCmd {
    /// Delete a retired device's data from the remote so its old files stop coming back
    Remove { name: String },
}

#[derive(Subcommand)]
enum CollectionCmd {
    /// Add a directory to sync (same name must be added on every device)
    Add {
        name: String,
        /// Absolute path or ~/path
        path: String,
        /// Relative sub-paths to skip (repeatable)
        #[arg(long)]
        exclude: Vec<String>,
    },
    /// Remove a collection from this device's config and sync state (remote data is kept)
    Remove {
        name: String,
    },
    List,
}

fn log(msg: &str) {
    use std::os::unix::fs::OpenOptionsExt;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(home().join("engram.log"))
    {
        let _ = writeln!(
            f,
            "{} {}",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
            msg
        );
    }
}

/// Serialize concurrent hook runs (async PostToolUse + Stop) on state.json
struct Lock(std::path::PathBuf);
impl Lock {
    fn acquire() -> Result<Lock> {
        Self::acquire_within(100)
    }

    /// Try for `tries` x 100ms; per-prompt pulls must give up well inside the hook timeout
    fn acquire_within(tries: u32) -> Result<Lock> {
        let p = home().join("lock");
        for _ in 0..tries {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&p)
            {
                Ok(_) => return Ok(Lock(p)),
                Err(_) => {
                    if let Ok(md) = std::fs::metadata(&p)
                        && md
                            .modified()
                            .map(|t| t.elapsed().unwrap_or_default().as_secs() > 60)
                            .unwrap_or(false)
                    {
                        let _ = std::fs::remove_file(&p);
                        continue;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            }
        }
        bail!("lock busy")
    }
}
impl Drop for Lock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn cols<'a>(cfg: &'a Config, only: Option<&str>) -> Vec<&'a Collection> {
    cfg.collections
        .iter()
        .filter(|c| only.is_none_or(|o| o == c.name))
        .collect()
}

async fn open(cfg: &Config) -> Result<(Keys, Backend)> {
    Ok((Keys::load_or_create()?, Backend::open(cfg).await?))
}

/// What a pull/push wants to say: lines for Claude/the terminal, timing parts for the hook status line
#[derive(Default)]
struct Out {
    lines: Vec<String>,
    timing: Vec<String>,
}

async fn do_pull(cfg: &Config, only: Option<&str>, cached_list: bool, out: &mut Out) -> Result<()> {
    let (keys, be) = open(cfg).await?;
    let mut s = Syncer::new(cfg, &keys, &be)?;
    s.cached_list = cached_list;
    let mut err = None;
    for (c, r) in s.pull_all(&cols(cfg, only)).await {
        let r = match r {
            Ok(r) => r,
            Err(e) => {
                out.timing.push(format!("{} failed", c.name));
                err.get_or_insert(e.context(format!("pull {}", c.name)));
                continue;
            }
        };
        let what = match r.changes.len() {
            0 => "up to date".to_string(),
            n => format!("{n} file(s)"),
        };
        out.timing.push(format!("{} {}ms ({what})", c.name, r.ms));
        for sk in &r.skipped {
            log(&format!("pull {}: skipped {sk}", c.name));
            out.timing.push(format!("{} skipped {sk}", c.name));
        }
        let mut by_dev: std::collections::BTreeMap<&str, usize> = Default::default();
        for ch in &r.changes {
            *by_dev.entry(ch.device.as_str()).or_default() += 1;
            log(&format!(
                "pull {}:{} {} from {}",
                c.name, ch.path, ch.kind, ch.device
            ));
        }
        for (d, n) in by_dev {
            out.lines
                .push(format!("engram: pulled {n} {} file(s) from {d}", c.name));
        }
        for ch in r.changes.iter().filter(|c| c.kind == "conflict") {
            out.lines.push(format!("engram: CONFLICT {}", ch.path));
        }
        if !r.new_index_lines.is_empty() {
            out.lines
                .push("engram: new memory index entries from other devices:".into());
            out.lines.extend(r.new_index_lines);
        }
    }
    err.map_or(Ok(()), Err)
}

async fn do_push(cfg: &Config, only: Option<&str>, out: &mut Out) -> Result<()> {
    let (keys, be) = open(cfg).await?;
    let mut s = Syncer::new(cfg, &keys, &be)?;
    let plugins_sha = s.state.plugins_sha.clone();
    // Plugin list upload overlaps the collection uploads instead of trailing them
    let plugins = async {
        let t = std::time::Instant::now();
        let r = match only {
            None => plugins::publish(&be, &keys, &cfg.device, plugins_sha.as_deref()).await,
            Some(_) => Ok(None),
        };
        (r, t.elapsed().as_millis())
    };
    let targets = cols(cfg, only);
    let ((done, err), (plugins, plugins_ms)) = tokio::join!(s.push_all(&targets), plugins);
    for p in done {
        log(&format!("push {} {} change(s)", p.col.name, p.changed));
        out.lines.push(format!(
            "engram: pushed {} {} change(s)",
            p.changed, p.col.name
        ));
        out.timing.push(format!(
            "{} {}ms ({} change(s), {} blob(s))",
            p.col.name, p.ms, p.changed, p.blobs
        ));
    }
    if let Some(sha) = plugins? {
        s.state.plugins_sha = Some(sha);
        s.state.save()?;
        log("push plugin list");
        out.timing.push(format!("plugins {plugins_ms}ms"));
    }
    err.map_or(Ok(()), Err)
}

/// CLI flavour: print pull lines, or "up to date" when nothing came in
async fn cli_pull(cfg: &Config, only: Option<&str>) -> Result<()> {
    let mut out = Out::default();
    // CLI runs take the same lock as hooks so they never interleave on state.json
    let _lock = Lock::acquire()?;
    let r = do_pull(cfg, only, false, &mut out).await;
    for t in out.timing.iter().filter(|t| t.contains(" skipped ")) {
        eprintln!("engram: warning: {t}");
    }
    print_lines(&out.lines, "engram: up to date");
    r
}

async fn cli_push(cfg: &Config, only: Option<&str>) -> Result<()> {
    let mut out = Out::default();
    // CLI runs take the same lock as hooks so they never interleave on state.json
    let _lock = Lock::acquire()?;
    let r = do_push(cfg, only, &mut out).await;
    print_lines(&out.lines, "");
    r
}

fn print_lines(lines: &[String], empty: &str) {
    if lines.is_empty() && !empty.is_empty() {
        println!("{empty}");
    }
    for l in lines {
        println!("{l}");
    }
}

async fn plugins_diff(cfg: &Config) -> Result<()> {
    let (keys, be) = open(cfg).await?;
    let mut s = Syncer::new(cfg, &keys, &be)?;
    if let Some(sha) =
        plugins::publish(&be, &keys, &cfg.device, s.state.plugins_sha.as_deref()).await?
    {
        s.state.plugins_sha = Some(sha);
        s.state.save()?;
    }
    let mine = plugins::local_list()?;
    let others = plugins::fetch_all(&be, &keys, &cfg.device).await?;
    plugins::print_diff(&cfg.device, &mine, &others);
    Ok(())
}

fn collection_cmd(cmd: CollectionCmd) -> Result<()> {
    let mut cfg = Config::load()?;
    match cmd {
        CollectionCmd::Add {
            name,
            path,
            exclude,
        } => {
            if cfg.collection(&name).is_some() {
                bail!("collection {name} already exists");
            }
            if !expand(&path).is_dir() {
                bail!("{path} is not a directory");
            }
            let mut exclude = exclude;
            // Hooks in a synced settings.json would run on every device
            if path.trim_end_matches('/').ends_with(".claude") {
                for f in ["settings.json", "settings.local.json"] {
                    if !exclude.iter().any(|x| x == f) {
                        exclude.push(f.into());
                    }
                }
            }
            cfg.collections.push(Collection {
                name: name.clone(),
                path,
                exclude,
            });
            cfg.save()?;
            println!("added {name}; run `engram sync` (and add it on the other devices too)");
        }
        CollectionCmd::Remove { name } => {
            let before = cfg.collections.len();
            cfg.collections.retain(|c| c.name != name);
            if cfg.collections.len() == before {
                bail!("no collection {name}");
            }
            // Stale own manifest would push tombstones for every file if the name is re-added
            let _lock = Lock::acquire()?;
            let mut st = model::State::load()?;
            st.forget(&name);
            st.save()?;
            cfg.save()?;
            println!("removed {name} from this device's config");
        }
        CollectionCmd::List => {
            for c in &cfg.collections {
                let ex = if c.exclude.is_empty() {
                    String::new()
                } else {
                    format!("  exclude {}", c.exclude.join(","))
                };
                println!("{:<12} {}{ex}", c.name, c.path);
            }
        }
    }
    Ok(())
}

async fn status(cfg: &Config) -> Result<()> {
    let (keys, be) = open(cfg).await?;
    let s = Syncer::new(cfg, &keys, &be)?;
    println!(
        "device: {}  remote: {}  home: {}",
        cfg.device,
        cfg.remote,
        home().display()
    );
    println!("identity public: {}", keys.public());
    for c in &cfg.collections {
        let own = s.state.own.get(&c.name).cloned().unwrap_or_default();
        let local = sync::scan(c)?;
        let pending = local
            .iter()
            .filter(|(k, l)| {
                own.files.get(*k).and_then(|e| e.sha.as_deref()) != Some(l.sha.as_str())
            })
            .count()
            + own
                .files
                .iter()
                .filter(|(k, e)| e.sha.is_some() && !local.contains_key(*k))
                .count();
        println!(
            "[{}] {}  files={} own.seq={} pending={}",
            c.name,
            expand(&c.path).display(),
            local.len(),
            own.seq,
            pending
        );
        for (dev, ds) in s.state.devices.get(&c.name).into_iter().flatten() {
            println!("    seen {dev} seq={}", ds.seq);
        }
    }
    Ok(())
}

/// Delete `<col>/<name>/` for every collection plus `plugins/<name>/`, and forget the device locally
async fn device_remove(cfg: &Config, name: &str) -> Result<()> {
    if !safety::valid_device(name) {
        bail!("invalid device name {name:?}");
    }
    if name == cfg.device {
        bail!("{name} is this device; run `engram device remove` from another device");
    }
    let (_keys, be) = open(cfg).await?;
    let _lock = Lock::acquire()?;
    let mut n = 0;
    let prefixes = cfg
        .collections
        .iter()
        .map(|c| format!("{}/{name}/", c.name))
        .chain([format!("plugins/{name}/")]);
    for prefix in prefixes {
        for k in be.list(&prefix).await? {
            be.delete(&k).await?;
            n += 1;
        }
    }
    let mut st = model::State::load()?;
    st.forget_device(name);
    st.save()?;
    println!("removed {n} object(s) of {name}; other devices drop it on their next LIST");
    Ok(())
}

/// `hostname -s`, restricted to [A-Za-z0-9._-] so it is a safe S3 key segment
fn default_device() -> Result<String> {
    let out = std::process::Command::new("hostname")
        .arg("-s")
        .output()
        .context("hostname -s")?;
    let raw = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let name: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "._-".contains(c) {
                c
            } else {
                '-'
            }
        })
        .collect();
    if name.is_empty() {
        bail!("cannot derive device name from hostname; pass --device");
    }
    Ok(name)
}

async fn init(
    device: Option<String>,
    remote: String,
    endpoint: Option<String>,
    access_key: Option<String>,
    secret_key: Option<String>,
    import: Option<String>,
    no_skills: bool,
) -> Result<()> {
    if Config::path().exists() {
        bail!("{} exists; remove it to re-init", Config::path().display());
    }
    let device = match device {
        Some(d) => d,
        None => default_device()?,
    };
    if !safety::valid_device(&device) {
        bail!("device name must match [A-Za-z0-9._-]{{1,64}}");
    }
    // State from an earlier setup would push tombstones for files the new remote never had
    let st = home().join("state.json");
    if st.exists() {
        let bak = home().join(format!("state.json.bak-{}", chrono::Utc::now().timestamp()));
        std::fs::rename(&st, &bak)?;
        println!("moved old sync state to {}", bak.display());
    }
    println!("device: {device}");
    let mem_dir = home().join("memory");
    std::fs::create_dir_all(&mem_dir)?;
    let mut collections = vec![Collection {
        name: "memory".into(),
        path: "~/.engram/memory".into(),
        exclude: vec![],
    }];
    if std::env::var("ENGRAM_HOME").is_ok() {
        collections[0].path = mem_dir.display().to_string();
    }
    if !no_skills {
        collections.push(Collection {
            name: "skills".into(),
            path: "~/.claude/skills".into(),
            exclude: vec!["synced".into()],
        });
    }
    let cfg = Config {
        device,
        remote,
        endpoint,
        region: Some("auto".into()),
        access_key_id: access_key,
        secret_access_key: secret_key,
        collections,
        hook_timing: false,
        device_list_ttl: 600,
        background_pull: true,
    };
    // ENGRAM_IDENTITY joins an existing remote without copying identity.txt by hand
    let id_path = home().join("identity.txt");
    if let Ok(id) = std::env::var("ENGRAM_IDENTITY")
        && !id_path.exists()
    {
        let id = id.trim();
        if !id.starts_with("AGE-SECRET-KEY-") {
            bail!("ENGRAM_IDENTITY must be an AGE-SECRET-KEY-… line");
        }
        config::write_private(&id_path, format!("{id}\n").as_bytes())?;
    }
    cfg.save()?;
    let keys = Keys::load_or_create()?;
    println!(
        "config: {}\nidentity: {} (public {})",
        Config::path().display(),
        home().join("identity.txt").display(),
        keys.public()
    );

    let src = import.map(|p| expand(&p)).or_else(default_project_memory);
    if let Some(src) = src.filter(|s| s.is_dir()) {
        let mut n = 0;
        for e in std::fs::read_dir(&src)? {
            let e = e?;
            if e.file_type()?.is_file() {
                let dst = mem_dir.join(e.file_name());
                if !dst.exists() {
                    std::fs::copy(e.path(), dst)?;
                    n += 1;
                }
            }
        }
        println!("imported {n} file(s) from {}", src.display());
    }
    cli_pull(&cfg, None).await?;
    cli_push(&cfg, None).await?;
    println!(
        "done. other devices: same init with this identity (copy identity.txt or set ENGRAM_IDENTITY)"
    );
    Ok(())
}

/// Claude's memory dir for the current directory, outside any custom setting
fn default_project_memory() -> Option<std::path::PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    let name: String = cwd
        .display()
        .to_string()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let dir = dirs::home_dir()?
        .join(".claude/projects")
        .join(name)
        .join("memory");
    dir.is_dir().then_some(dir)
}

/// Per-prompt pull of `memory` only, skipping when a push holds the lock
async fn prompt_pull(cfg: &Config, out: &mut Out) -> Result<()> {
    // Busy = a push is running; skip this prompt instead of stalling it
    let Ok(_lock) = Lock::acquire_within(10) else {
        out.timing.push("skipped, push in progress".into());
        return Ok(());
    };
    // Only memory surfaces index lines mid-session; other collections wait for SessionStart
    let only = cfg.collection("memory").map(|c| c.name.as_str());
    do_pull(cfg, only, true, out).await
}

fn inbox_dir() -> std::path::PathBuf {
    home().join("inbox")
}

/// Drain results left by background pulls, oldest first
fn inbox_take(out: &mut Out) {
    let Ok(rd) = std::fs::read_dir(inbox_dir()) else {
        return;
    };
    let mut files: Vec<_> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort();
    for f in files {
        let got = std::fs::read(&f)
            .ok()
            .and_then(|d| serde_json::from_slice::<(Vec<String>, Vec<String>)>(&d).ok());
        let _ = std::fs::remove_file(&f);
        if let Some((lines, timing)) = got {
            out.lines.extend(lines);
            out.timing
                .extend(timing.into_iter().map(|t| format!("bg {t}")));
        }
    }
}

/// One file per background run: writers never touch each other's results
fn inbox_put(out: &Out) -> Result<()> {
    if out.lines.is_empty() && out.timing.is_empty() {
        return Ok(());
    }
    let dir = inbox_dir();
    std::fs::create_dir_all(&dir)?;
    let name = format!(
        "{}-{}",
        chrono::Utc::now().format("%Y%m%dT%H%M%S%.6f"),
        std::process::id()
    );
    config::write_private(
        &dir.join(format!("{name}.json")),
        &serde_json::to_vec(&(&out.lines, &out.timing))?,
    )?;
    Ok(())
}

/// Detached `engram hook prompt-bg`: own pgroup, no stdio, hook returns now
fn spawn_bg() -> Result<()> {
    use std::os::unix::process::CommandExt;
    std::process::Command::new(std::env::current_exe()?)
        .args(["hook", "prompt-bg"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0)
        .spawn()?;
    Ok(())
}

/// Run one hook event; fills `out` and returns the action label for the status line
async fn hook(
    event: &str,
    input: &hooks::HookInput,
    cfg: &Config,
    out: &mut Out,
    t: &mut std::time::Instant,
) -> Result<&'static str> {
    match event {
        "userpromptsubmit" if cfg.background_pull => {
            inbox_take(out);
            spawn_bg()?;
            return Ok("pull");
        }
        "userpromptsubmit" => return prompt_pull(cfg, out).await.map(|_| "pull"),
        "prompt-bg" => {
            if let Err(e) = prompt_pull(cfg, out).await {
                log(&format!("hook prompt-bg failed: {e:#}"));
                out.timing.push(format!("failed: {e:#}"));
            }
            inbox_put(out)?;
            return Ok("");
        }
        _ => {}
    }
    let _lock = Lock::acquire()?;
    match event {
        "sessionstart" => {
            do_pull(cfg, None, false, out).await?;
            if input.codex {
                codex_memory(cfg, out);
            }
            Ok("pull")
        }
        "posttooluse" => {
            let mut cols: Vec<&str> = input
                .files
                .iter()
                .filter_map(|f| cfg.collection_for(f).map(|c| c.name.as_str()))
                .collect();
            cols.dedup();
            if cols.is_empty() {
                return Ok("");
            }
            // Let a Write+Edit pair land in one manifest
            tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
            *t = std::time::Instant::now();
            let only = (cols.len() == 1).then_some(cols[0]);
            do_push(cfg, only, out).await.map(|_| "push")
        }
        "stop" => do_push(cfg, None, out).await.map(|_| "push"),
        other => bail!("unknown hook event {other}"),
    }
}

/// Codex has no autoMemoryDirectory: hand it the memory dir and MEMORY.md at session start
fn codex_memory(cfg: &Config, out: &mut Out) {
    let Some(mem) = cfg.collection("memory") else {
        return;
    };
    let dir = expand(&mem.path);
    let index = std::fs::read_to_string(dir.join("MEMORY.md")).unwrap_or_default();
    let index: Vec<&str> = index.lines().take(200).collect();
    out.lines.push(format!(
        "# engram memory\nPersistent memory shared with Claude Code lives in {}. \
         Read a topic file there when its index line below is relevant. \
         To save or update a memory, follow the `engram` skill.\n\n{}",
        dir.display(),
        index.join("\n")
    ));
}

/// Session start before `engram init`: tell the user and the agent how to finish setup
fn not_set_up() -> String {
    serde_json::json!({
        "systemMessage": "engram: not set up yet. Run /engram:setup (Codex: ask for the engram setup skill).",
        "hookSpecificOutput": {
            "hookEventName": "SessionStart",
            "additionalContext": "The engram plugin is installed but not configured (no ~/.engram/config.json), so memory is not synced. If the user asks about engram, memory sync or setup, use the engram `setup` skill."
        }
    })
    .to_string()
}

/// Hook stdout as Claude Code JSON: `systemMessage` shows timings to the user, `additionalContext` feeds Claude
fn hook_output(event: &str, cfg: &Config, action: &str, ms: u128, out: &Out) -> Option<String> {
    let mut v = serde_json::Map::new();
    // Failures always surface, even with timings hidden
    let alert = out
        .timing
        .iter()
        .any(|t| t.contains("failed") || t.contains(" skipped "));
    if (cfg.hook_timing || alert) && !action.is_empty() {
        let mut msg = format!("engram {action} {ms}ms");
        if out.timing.is_empty() {
            msg.push_str(" · no change");
        }
        for t in &out.timing {
            msg.push_str(" · ");
            msg.push_str(t);
        }
        v.insert("systemMessage".into(), msg.into());
    }
    let name = match event {
        "userpromptsubmit" => Some("UserPromptSubmit"),
        "sessionstart" => Some("SessionStart"),
        _ => None,
    };
    if let Some(name) = name
        && !out.lines.is_empty()
    {
        v.insert(
            "hookSpecificOutput".into(),
            serde_json::json!({ "hookEventName": name, "additionalContext": out.lines.join("\n") }),
        );
    }
    (!v.is_empty()).then(|| serde_json::Value::Object(v).to_string())
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let cli = Cli::parse();
    let res = match cli.cmd {
        Cmd::Init {
            device,
            remote,
            endpoint,
            access_key,
            secret_key,
            import,
            no_skills,
            plugin,
        } => init(
            device, remote, endpoint, access_key, secret_key, import, no_skills,
        )
        .await
        .and_then(|_| {
            if plugin {
                hooks::install(&Config::load()?, true)?;
                println!("autoMemoryDirectory set; plugin hooks start with the next session");
            }
            Ok(())
        }),
        Cmd::Pull { collection } => {
            async { cli_pull(&Config::load()?, collection.as_deref()).await }.await
        }
        Cmd::Push {
            collection,
            if_path,
        } => {
            async {
                let cfg = Config::load()?;
                let only = match if_path {
                    Some(p) => match cfg.collection_for(&expand(&p)) {
                        Some(c) => Some(c.name.clone()),
                        None => return Ok(()),
                    },
                    None => collection,
                };
                cli_push(&cfg, only.as_deref()).await
            }
            .await
        }
        Cmd::Sync => {
            async {
                let cfg = Config::load()?;
                cli_pull(&cfg, None).await?;
                cli_push(&cfg, None).await
            }
            .await
        }
        Cmd::Status => async { status(&Config::load()?).await }.await,
        Cmd::Device {
            cmd: DeviceCmd::Remove { name },
        } => async { device_remove(&Config::load()?, &name).await }.await,
        Cmd::InstallHooks { plugin } => Config::load()
            .and_then(|c| hooks::install(&c, plugin))
            .map(|_| {
                let what = if plugin {
                    "autoMemoryDirectory set"
                } else {
                    "hooks installed"
                };
                println!("{what} in {}", config::claude_settings_path().display())
            }),
        Cmd::HookTiming { state } => Config::load().and_then(|mut c| {
            c.hook_timing = state == "on";
            c.save()?;
            println!("hook timing {state}");
            Ok(())
        }),
        Cmd::UninstallHooks => hooks::uninstall().map(|_| println!("hooks removed")),
        Cmd::Collection { cmd } => collection_cmd(cmd),
        Cmd::Plugins => async { plugins_diff(&Config::load()?).await }.await,
        Cmd::Hook { event } => {
            // Hooks never block Claude: log, report, exit 0
            let mut t = std::time::Instant::now();
            let mut out = Out::default();
            let input = hooks::read_input();
            match Config::load() {
                Ok(cfg) => {
                    let action = match hook(&event, &input, &cfg, &mut out, &mut t).await {
                        Ok(a) => a,
                        Err(e) => {
                            log(&format!("hook {event} failed: {e:#}"));
                            out.timing.push(format!("failed: {e:#}"));
                            "hook"
                        }
                    };
                    if let Some(j) =
                        hook_output(&event, &cfg, action, t.elapsed().as_millis(), &out)
                    {
                        println!("{j}");
                    }
                }
                Err(_) if !Config::path().exists() => {
                    if event == "sessionstart" {
                        println!("{}", not_set_up());
                    }
                }
                Err(e) => log(&format!("hook {event} failed: {e:#}")),
            }
            Ok(())
        }
    };
    if let Err(e) = res.context("engram") {
        eprintln!("{e:#}");
        std::process::exit(1);
    }
}
