mod backend;
mod config;
mod crypto;
mod hooks;
mod memorymd;
mod model;
mod plugins;
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
        #[arg(long)]
        remote: String,
        #[arg(long)]
        endpoint: Option<String>,
        #[arg(long)]
        access_key: Option<String>,
        #[arg(long)]
        secret_key: Option<String>,
        /// Memory dir to copy into ~/.engram/memory (default: Claude's current project memory)
        #[arg(long)]
        import: Option<String>,
        /// Skip syncing ~/.claude/skills
        #[arg(long)]
        no_skills: bool,
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
    InstallHooks,
    UninstallHooks,
    /// Entry points called by Claude Code hooks (fail soft, log to engram.log)
    Hook {
        event: String,
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
    /// Remove a collection from this device's config (remote data is kept)
    Remove {
        name: String,
    },
    List,
}

fn log(msg: &str) {
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
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
        let p = home().join("lock");
        for _ in 0..100 {
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

async fn do_pull(cfg: &Config, only: Option<&str>, quiet_if_empty: bool) -> Result<()> {
    let (keys, be) = open(cfg).await?;
    let mut s = Syncer::new(cfg, &keys, &be)?;
    let mut any = false;
    for c in cols(cfg, only) {
        let r = s.pull(c).await?;
        if r.changes.is_empty() {
            continue;
        }
        any = true;
        let mut by_dev: std::collections::BTreeMap<&str, usize> = Default::default();
        for ch in &r.changes {
            *by_dev.entry(ch.device.as_str()).or_default() += 1;
            log(&format!(
                "pull {}:{} {} from {}",
                c.name, ch.path, ch.kind, ch.device
            ));
        }
        for (d, n) in by_dev {
            println!("engram: pulled {n} {} file(s) from {d}", c.name);
        }
        for ch in r.changes.iter().filter(|c| c.kind == "conflict") {
            println!("engram: CONFLICT {}", ch.path);
        }
        if !r.new_index_lines.is_empty() {
            println!("engram: new memory index entries from other devices:");
            for l in r.new_index_lines {
                println!("{l}");
            }
        }
    }
    if !any && !quiet_if_empty {
        println!("engram: up to date");
    }
    Ok(())
}

async fn do_push(cfg: &Config, only: Option<&str>, quiet: bool) -> Result<()> {
    let (keys, be) = open(cfg).await?;
    let mut s = Syncer::new(cfg, &keys, &be)?;
    for c in cols(cfg, only) {
        let n = s.push(c).await?;
        if n > 0 {
            log(&format!("push {} {n} change(s)", c.name));
            if !quiet {
                println!("engram: pushed {n} {} change(s)", c.name);
            }
        }
    }
    if only.is_none()
        && let Some(sha) =
            plugins::publish(&be, &keys, &cfg.device, s.state.plugins_sha.as_deref()).await?
    {
        s.state.plugins_sha = Some(sha);
        s.state.save()?;
        log("push plugin list");
    }
    Ok(())
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
    };
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
    do_pull(&cfg, None, false).await?;
    do_push(&cfg, None, false).await?;
    println!(
        "done. next: `engram install-hooks`, then copy identity.txt + config.json to other devices"
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

async fn hook(event: &str) -> Result<()> {
    let cfg = Config::load()?;
    let _lock = Lock::acquire()?;
    match event {
        "sessionstart" => do_pull(&cfg, None, true).await,
        "userpromptsubmit" => do_pull(&cfg, None, true).await,
        "posttooluse" => {
            let Some(fp) = hooks::stdin_file_path() else {
                return Ok(());
            };
            let Some(col) = cfg.collection_for(&fp) else {
                return Ok(());
            };
            // Let a Write+Edit pair land in one manifest
            tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
            do_push(&cfg, Some(&col.name), true).await
        }
        "stop" => do_push(&cfg, None, true).await,
        other => bail!("unknown hook event {other}"),
    }
}

#[tokio::main]
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
        } => {
            init(
                device, remote, endpoint, access_key, secret_key, import, no_skills,
            )
            .await
        }
        Cmd::Pull { collection } => {
            async { do_pull(&Config::load()?, collection.as_deref(), false).await }.await
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
                do_push(&cfg, only.as_deref(), false).await
            }
            .await
        }
        Cmd::Sync => {
            async {
                let cfg = Config::load()?;
                do_pull(&cfg, None, false).await?;
                do_push(&cfg, None, false).await
            }
            .await
        }
        Cmd::Status => async { status(&Config::load()?).await }.await,
        Cmd::InstallHooks => Config::load().and_then(|c| hooks::install(&c)).map(|_| {
            println!(
                "hooks installed in {}",
                config::claude_settings_path().display()
            )
        }),
        Cmd::UninstallHooks => hooks::uninstall().map(|_| println!("hooks removed")),
        Cmd::Collection { cmd } => collection_cmd(cmd),
        Cmd::Plugins => async { plugins_diff(&Config::load()?).await }.await,
        Cmd::Hook { event } => {
            // Hooks never block Claude: log and exit 0
            if let Err(e) = hook(&event).await {
                log(&format!("hook {event} failed: {e:#}"));
            }
            Ok(())
        }
    };
    if let Err(e) = res.context("engram") {
        eprintln!("{e:#}");
        std::process::exit(1);
    }
}
