mod books;
mod classifier;
mod config;
mod docmeta;
mod media;
mod models;
mod organize;
mod reload;
mod service;
mod store;
mod wiki;

use anyhow::{Context, Result};
use chrono::Local;
use clap::{Parser, Subcommand};
use config::Config;
use croner::Cron;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::Mutex;
use std::time::Duration;

const ABOUT: &str =
    "Sorts the files in a watch folder (e.g. Downloads) into a library: movies, TV series, \
music, pictures and books, identified with Wikipedia and a classifier trained on Wikipedia.";

const LONG_ABOUT: &str = "\
filesort watches one folder (watch_dir) and moves what it finds into your library:

  Movies/<Title (Year)>/            Series/<Show>/Season 01/
  Music/<Artist>/<Album>/           Pictures/<Year>/<Album>/
  <Subject>/  (books, e.g. Fiction/, Math/, Papers/)

Files are identified from their name, their own metadata (EPUB / PDF / audio tags /
EXIF) and Wikipedia. Book subjects and genres come from a Naive Bayes classifier
trained on Wikipedia. When it isn't sure enough, a file is left where it is.

It is safe to try: dry_run = true in the config only logs what would happen.";

const EXAMPLES: &str = "\
Getting started:
  filesort init                     write a starter config to the default location
  filesort scan --dry-run           one pass, only show what would be moved
  filesort --now                    keep running: scan now, then on the schedule
  filesort service install          run in the background, start with the computer

Other commands:
  filesort scan                     one pass, then exit (for cron / Task Scheduler)
  filesort evaluate                 how accurate the classifier is
  filesort service status|stop|start|uninstall
  filesort config                   show where the config and training data are

Files:
  Config:         ~/.config/filesort/filesort.toml (or $XDG_CONFIG_HOME/filesort,
                  or --config / $FILESORT_CONFIG)
  Training data:  training.bin next to the config (a built-in copy is used if missing)

Changes to the config file are applied while filesort runs; no restart needed.";

#[derive(Parser)]
#[command(name = "filesort", version, about = ABOUT, long_about = LONG_ABOUT, after_help = EXAMPLES)]
struct Cli {
    /// Config file [default: ~/.config/filesort/filesort.toml]
    #[arg(
        short,
        long,
        global = true,
        env = "FILESORT_CONFIG",
        value_name = "PATH"
    )]
    config: Option<PathBuf>,
    /// Only log what would happen, whatever the config says
    #[arg(long, global = true, conflicts_with = "move")]
    dry_run: bool,
    /// Really move files, whatever the config says
    #[arg(long = "move", global = true)]
    r#move: bool,
    /// Scan immediately on start, then follow the schedule
    #[arg(long)]
    now: bool,
    /// Same as `filesort scan` (kept for scripts)
    #[arg(long, hide = true)]
    once: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Keep running and scan on the schedule from the config (the default)
    Run {
        /// Scan immediately on start, then follow the schedule
        #[arg(long)]
        now: bool,
    },
    /// Scan once and exit
    Scan,
    /// Write a commented starter config (default location unless PATH is given)
    Init {
        /// Where to write it [default: ~/.config/filesort/filesort.toml]
        path: Option<PathBuf>,
        /// Overwrite an existing file
        #[arg(long)]
        force: bool,
    },
    /// Show how accurate the classifier is on held-out training data
    Evaluate,
    /// Install or control filesort as a background service (Windows service,
    /// systemd user service on Linux, launchd agent on macOS)
    Service {
        #[arg(value_enum)]
        action: service::Action,
    },
    /// Show where the config, training data and log are
    Config,
    /// Entry point used by the Windows service manager
    #[command(hide = true)]
    ServiceRun,
}

static LOG_FILE: Mutex<Option<std::fs::File>> = Mutex::new(None);

pub fn log(msg: &str) {
    let line = format!("{} {msg}", Local::now().format("%Y-%m-%d %H:%M:%S"));
    println!("{line}");
    if let Ok(mut f) = LOG_FILE.lock() {
        if let Some(f) = f.as_mut() {
            let _ = writeln!(f, "{line}");
        }
    }
}

/// `~/.config/filesort` (or `$XDG_CONFIG_HOME/filesort`) on every OS.
pub fn config_dir() -> PathBuf {
    let base = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(d) if !d.is_empty() => PathBuf::from(d),
        _ => dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".config"),
    };
    base.join("filesort")
}

fn default_config_path() -> PathBuf {
    config_dir().join("filesort.toml")
}

fn config_path(cli: &Cli) -> Result<PathBuf> {
    let path = cli.config.clone().unwrap_or_else(default_config_path);
    if !path.exists() {
        anyhow::bail!(
            "no config at {}\nCreate one with `filesort init`{}",
            path.display(),
            if cli.config.is_some() {
                format!(" {}", path.display())
            } else {
                String::new()
            }
        );
    }
    Ok(path)
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let dry_override = if cli.dry_run {
        Some(true)
    } else if cli.r#move {
        Some(false)
    } else {
        None
    };

    match &cli.command {
        Some(Command::Init { path, force }) => {
            let path = path
                .clone()
                .or(cli.config.clone())
                .unwrap_or_else(default_config_path);
            if path.exists() && !force {
                anyhow::bail!(
                    "{} already exists (use --force to overwrite)",
                    path.display()
                );
            }
            if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(&path, config::sample())?;
            println!("wrote {}", path.display());
            println!("Check watch_dir and library_root in it, then try:  filesort scan --dry-run");
            return Ok(());
        }
        Some(Command::Config) => {
            let path = cli.config.clone().unwrap_or_else(default_config_path);
            println!(
                "config:        {}{}",
                path.display(),
                if path.exists() {
                    ""
                } else {
                    "  (missing: run `filesort init`)"
                }
            );
            if let Ok(text) = std::fs::read_to_string(&path) {
                if let Ok(cfg) = Config::parse(&text) {
                    let tf = models::resolve_training_file(&cfg.classifier.training_file, &path);
                    println!(
                        "training data: {}{}",
                        tf.display(),
                        if tf.exists() {
                            ""
                        } else {
                            "  (not yet created; the built-in copy is used)"
                        }
                    );
                    println!("watch folder:  {}", cfg.watch_dir.display());
                    println!("library:       {}", cfg.library_root.display());
                    if !cfg.log_file.trim().is_empty() {
                        println!(
                            "log file:      {}",
                            resolve(&path, Path::new(cfg.log_file.trim())).display()
                        );
                    }
                }
            }
            return Ok(());
        }
        Some(Command::Service { action }) => return service::control(*action, &config_path(&cli)?),
        Some(Command::Evaluate) => {
            let path = config_path(&cli)?;
            let mut cfg = Config::parse(&std::fs::read_to_string(&path)?)?;
            cfg.classifier.training_file =
                models::resolve_training_file(&cfg.classifier.training_file, &path);
            models::evaluate(&cfg.classifier);
            return Ok(());
        }
        Some(Command::ServiceRun) => {
            #[cfg(windows)]
            {
                let path = config_path(&cli)?;
                return service::run_as_service(Box::new(move |stop: &AtomicBool| {
                    if let Err(e) = daemon(&path, dry_override, true, stop, true) {
                        log(&format!("stopped: {e:#}"));
                    }
                }));
            }
            #[cfg(not(windows))]
            anyhow::bail!("service-run is only used by the Windows service manager");
        }
        Some(Command::Scan) => {
            let path = config_path(&cli)?;
            let mut active = Active::load(&path, dry_override, false)?;
            active.announce(&path);
            active.pass();
            return Ok(());
        }
        Some(Command::Run { .. }) | None => {}
    }

    let path = config_path(&cli)?;
    if cli.once {
        let mut active = Active::load(&path, dry_override, false)?;
        active.announce(&path);
        active.pass();
        return Ok(());
    }
    let now = cli.now || matches!(cli.command, Some(Command::Run { now: true }));
    daemon(&path, dry_override, now, &AtomicBool::new(false), false)
}

/// Resolve a path from the config against the config's folder.
fn resolve(config_path: &Path, p: &Path) -> PathBuf {
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        config_path
            .parent()
            .filter(|d| !d.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
            .join(p)
    }
}

/// Run on the schedule until `stop` is set. Config edits are applied as they're saved.
fn daemon(
    config_path: &Path,
    dry_override: Option<bool>,
    now: bool,
    stop: &AtomicBool,
    as_service: bool,
) -> Result<()> {
    let mut active = Active::load(config_path, dry_override, as_service)?;
    active.announce(config_path);

    // Config changes arrive as events; a failure to watch is not fatal, the file is also
    // re-checked on every wake-up below.
    let watcher = match reload::watch(config_path) {
        Ok(w) => Some(w),
        Err(e) => {
            log(&format!(
                "config changes will be noticed within 30 s only: {e:#}"
            ));
            None
        }
    };

    if now || active.run_on_start {
        active.pass();
    }
    'schedule: while !stop.load(Ordering::SeqCst) {
        let next = active
            .cron
            .find_next_occurrence(&Local::now(), false)
            .context("schedule has no upcoming run time")?;
        log(&format!("next scan at {}", next.format("%Y-%m-%d %H:%M")));
        let mut since_check = Duration::ZERO;
        loop {
            if stop.load(Ordering::SeqCst) {
                break 'schedule;
            }
            let now = Local::now();
            if now >= next {
                break;
            }
            // Short waits: react quickly to a stop request, and don't miss a run after the
            // clock changes or the computer wakes from sleep.
            let wait = (next - now)
                .to_std()
                .unwrap_or_default()
                .min(Duration::from_secs(2));
            let changed = match &watcher {
                Some((_, rx)) => match rx.recv_timeout(wait) {
                    Ok(()) => {
                        // Editors fire several events per save; let them settle, then drain.
                        std::thread::sleep(Duration::from_millis(300));
                        while rx.try_recv().is_ok() {}
                        true
                    }
                    Err(RecvTimeoutError::Timeout) => false,
                    Err(RecvTimeoutError::Disconnected) => {
                        std::thread::sleep(wait);
                        false
                    }
                },
                None => {
                    std::thread::sleep(wait);
                    false
                }
            };
            since_check += wait;
            // Safety net every 30 s for tools whose saves don't produce a change event.
            if changed || since_check >= Duration::from_secs(30) {
                since_check = Duration::ZERO;
                if active.reload(config_path, dry_override, as_service) {
                    continue 'schedule; // new schedule -> recompute the next run
                }
            }
        }
        active.pass();
        if let Some((_, rx)) = &watcher {
            if rx.try_recv().is_ok() {
                while rx.try_recv().is_ok() {}
                active.reload(config_path, dry_override, as_service);
            }
        }
    }
    log("stopping");
    Ok(())
}

/// The settings currently in effect.
struct Active {
    /// Config text the current settings were built from.
    text: String,
    /// Last text seen on disk, including a broken one, so the same error isn't logged twice.
    seen: String,
    cron: Cron,
    sorter: organize::Sorter,
    run_on_start: bool,
    summary: String,
}

impl Active {
    fn load(path: &Path, dry_override: Option<bool>, as_service: bool) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading config {}", path.display()))?;
        let mut cfg =
            Config::parse(&text).with_context(|| format!("parsing config {}", path.display()))?;
        if let Some(d) = dry_override {
            cfg.dry_run = d;
        }
        cfg.classifier.training_file =
            models::resolve_training_file(&cfg.classifier.training_file, path);
        let cron = Cron::new(&cfg.schedule).parse().with_context(|| {
            format!("invalid schedule {:?} in {}", cfg.schedule, path.display())
        })?;
        // A service has no console: without a log_file it logs to filesort.log by the config.
        let log_file = match cfg.log_file.trim() {
            "" if as_service => Some(resolve(path, Path::new("filesort.log"))),
            "" => None,
            f => Some(resolve(path, Path::new(f))),
        };
        *LOG_FILE.lock().unwrap() = match &log_file {
            Some(f) => Some(
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(f)
                    .with_context(|| format!("opening log file {}", f.display()))?,
            ),
            None => None,
        };
        let summary = format!(
            "watching {} -> {} | schedule \"{}\" | {}",
            cfg.dirs().watch.display(),
            cfg.library_root.display(),
            cfg.schedule,
            if cfg.dry_run {
                "DRY RUN (set dry_run = false to move files)"
            } else {
                "moving files"
            }
        );
        let run_on_start = cfg.run_on_start;
        Ok(Self {
            seen: text.clone(),
            text,
            cron,
            sorter: organize::Sorter::new(cfg),
            run_on_start,
            summary,
        })
    }

    fn announce(&self, path: &Path) {
        log(&format!(
            "filesort {} | config {} | {}",
            env!("CARGO_PKG_VERSION"),
            path.display(),
            self.summary
        ));
    }

    /// Re-read the config. Applies it and returns true if it changed and is valid; a broken
    /// config is reported and the previous settings stay in effect.
    fn reload(&mut self, path: &Path, dry_override: Option<bool>, as_service: bool) -> bool {
        let Ok(text) = std::fs::read_to_string(path) else {
            log("config file unreadable (being saved?), keeping current settings");
            return false;
        };
        if text == self.seen {
            return false;
        }
        self.seen = text.clone();
        match Active::load(path, dry_override, as_service) {
            Ok(new) => {
                let changes = reload::changes(&self.text, &new.text);
                *self = new;
                log(&format!(
                    "config reloaded: {}",
                    if changes.is_empty() {
                        "formatting only".into()
                    } else {
                        changes.join(", ")
                    }
                ));
                log(&format!("now {}", self.summary));
                true
            }
            Err(e) => {
                let msg = format!("{e:#}");
                let msg: Vec<&str> = msg
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty() && *l != "|")
                    .collect();
                log(&format!(
                    "config error, keeping previous settings: {}",
                    msg.join(" ")
                ));
                false
            }
        }
    }

    fn pass(&mut self) {
        log("scan started");
        match self.sorter.run_once() {
            Ok(s) => log(&format!(
                "scan finished: {} sorted, {} skipped, {} deferred, {} failed",
                s.moved, s.skipped, s.deferred, s.failed
            )),
            Err(e) => log(&format!("scan failed: {e:#}")),
        }
    }
}
