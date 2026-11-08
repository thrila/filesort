//! Install / control filesort as a background service on Windows, Linux and macOS.
//!
//! - Windows: a Windows service ("FileSort", automatic start, runs as LocalSystem).
//! - Linux:   a systemd *user* unit, `~/.config/systemd/user/filesort.service`.
//! - macOS:   a launchd agent, `~/Library/LaunchAgents/com.filesort.agent.plist`.
//!
//! The service always gets the absolute path of the config file, so it finds the same
//! config and training data whichever account it runs under.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

pub const SERVICE_NAME: &str = "filesort";
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
pub const DISPLAY_NAME: &str = "FileSort";
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const LAUNCHD_LABEL: &str = "com.filesort.agent";

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum Action {
    /// Register the service and start it (it also starts at boot / login).
    Install,
    /// Stop and remove the service.
    Uninstall,
    /// Start the installed service.
    Start,
    /// Stop the running service.
    Stop,
    /// Show whether the service is installed and running.
    Status,
}

fn exe() -> Result<PathBuf> {
    let p = std::env::current_exe().context("finding this program's path")?;
    Ok(std::fs::canonicalize(&p).unwrap_or(p))
}

/// Strip Windows' `\\?\` prefix so paths look normal in service settings.
fn plain(p: &Path) -> String {
    let s = p.display().to_string();
    s.strip_prefix(r"\\?\").map(str::to_string).unwrap_or(s)
}

pub fn control(action: Action, config: &Path) -> Result<()> {
    let config = std::fs::canonicalize(config)
        .with_context(|| format!("config {} not found", config.display()))?;
    imp::control(action, &exe()?, &config)
}

// ---------------------------------------------------------------- Windows
#[cfg(windows)]
mod imp {
    use super::*;
    use std::ffi::OsString;
    use std::time::Duration;
    use windows_service::service::{
        ServiceAccess, ServiceErrorControl, ServiceInfo, ServiceStartType, ServiceState,
        ServiceType,
    };
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

    pub fn control(action: Action, exe: &Path, config: &Path) -> Result<()> {
        let need = match action {
            Action::Install => ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE,
            _ => ServiceManagerAccess::CONNECT,
        };
        let manager = ServiceManager::local_computer(None::<&str>, need).map_err(admin_hint)?;
        match action {
            Action::Install => {
                let info = ServiceInfo {
                    name: OsString::from(SERVICE_NAME),
                    display_name: OsString::from(DISPLAY_NAME),
                    service_type: ServiceType::OWN_PROCESS,
                    start_type: ServiceStartType::AutoStart,
                    error_control: ServiceErrorControl::Normal,
                    executable_path: PathBuf::from(plain(exe)),
                    launch_arguments: vec![
                        OsString::from("--config"),
                        OsString::from(plain(config)),
                        OsString::from("service-run"),
                    ],
                    dependencies: vec![],
                    account_name: None, // LocalSystem
                    account_password: None,
                };
                let svc = manager
                    .create_service(
                        &info,
                        ServiceAccess::CHANGE_CONFIG
                            | ServiceAccess::START
                            | ServiceAccess::QUERY_STATUS,
                    )
                    .map_err(admin_hint)?;
                let _ = svc.set_description(
                    "Sorts media from a watch folder into your library (filesort).",
                );
                svc.start::<&str>(&[]).context("starting the service")?;
                println!("installed and started the {DISPLAY_NAME} service");
                println!("  program: {}", plain(exe));
                println!("  config:  {}", plain(config));
                println!("It starts automatically with Windows. Logs: see log_file in the config (default filesort.log next to it).");
            }
            Action::Uninstall => {
                let svc = manager
                    .open_service(
                        SERVICE_NAME,
                        ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::DELETE,
                    )
                    .map_err(not_installed)?;
                if svc.query_status()?.current_state != ServiceState::Stopped {
                    let _ = svc.stop();
                    wait_for(&svc, ServiceState::Stopped);
                }
                svc.delete().context("removing the service")?;
                println!("removed the {DISPLAY_NAME} service");
            }
            Action::Start => {
                let svc = manager
                    .open_service(SERVICE_NAME, ServiceAccess::START)
                    .map_err(not_installed)?;
                svc.start::<&str>(&[]).context("starting the service")?;
                println!("started {DISPLAY_NAME}");
            }
            Action::Stop => {
                let svc = manager
                    .open_service(
                        SERVICE_NAME,
                        ServiceAccess::STOP | ServiceAccess::QUERY_STATUS,
                    )
                    .map_err(not_installed)?;
                svc.stop().context("stopping the service")?;
                wait_for(&svc, ServiceState::Stopped);
                println!("stopped {DISPLAY_NAME}");
            }
            Action::Status => match manager.open_service(SERVICE_NAME, ServiceAccess::QUERY_STATUS)
            {
                Ok(svc) => println!("{DISPLAY_NAME}: {:?}", svc.query_status()?.current_state),
                Err(_) => println!("{DISPLAY_NAME}: not installed"),
            },
        }
        Ok(())
    }

    fn wait_for(svc: &windows_service::service::Service, state: ServiceState) {
        for _ in 0..60 {
            if svc.query_status().is_ok_and(|s| s.current_state == state) {
                return;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }

    fn admin_hint(e: windows_service::Error) -> anyhow::Error {
        anyhow::anyhow!("{e} (installing a service needs an Administrator terminal: right-click it and choose \"Run as administrator\")")
    }

    fn not_installed(e: windows_service::Error) -> anyhow::Error {
        anyhow::anyhow!("{e} (is the service installed? run `filesort service install`)")
    }

    // --- running as a service ---

    windows_service::define_windows_service!(ffi_service_main, service_main);

    /// The scheduler loop; must return soon after the flag it gets becomes true.
    pub type RunFn = Box<dyn Fn(&std::sync::atomic::AtomicBool) + Send + Sync>;

    static RUN: std::sync::OnceLock<RunFn> = std::sync::OnceLock::new();

    /// Hand control to the Service Control Manager; `run` is the scheduler loop, which must
    /// return soon after the flag it's given becomes true.
    pub fn run_as_service(run: RunFn) -> Result<()> {
        let _ = RUN.set(run);
        windows_service::service_dispatcher::start(SERVICE_NAME, ffi_service_main)
            .context("this command is only for the Windows service manager; use `filesort` to run in a terminal")
    }

    fn service_main(_args: Vec<OsString>) {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        use windows_service::service::{
            ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceStatus,
        };
        use windows_service::service_control_handler::{self, ServiceControlHandlerResult};

        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let Ok(handle) = service_control_handler::register(SERVICE_NAME, move |c| match c {
            ServiceControl::Stop | ServiceControl::Shutdown => {
                flag.store(true, Ordering::SeqCst);
                ServiceControlHandlerResult::NoError
            }
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            _ => ServiceControlHandlerResult::NotImplemented,
        }) else {
            return;
        };
        let status = |state, accept| ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: state,
            controls_accepted: accept,
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::from_secs(10),
            process_id: None,
        };
        let _ = handle.set_service_status(status(
            ServiceState::Running,
            ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
        ));
        if let Some(run) = RUN.get() {
            run(&stop);
        }
        let _ =
            handle.set_service_status(status(ServiceState::Stopped, ServiceControlAccept::empty()));
    }
}

// ---------------------------------------------------------------- Linux (systemd)
#[cfg(all(unix, not(target_os = "macos")))]
mod imp {
    use super::*;
    use anyhow::bail;
    use std::process::Command;

    fn unit_path() -> Result<PathBuf> {
        let base = match std::env::var_os("XDG_CONFIG_HOME") {
            Some(d) if !d.is_empty() => PathBuf::from(d),
            _ => dirs::home_dir().context("no home folder")?.join(".config"),
        };
        Ok(base
            .join("systemd/user")
            .join(format!("{SERVICE_NAME}.service")))
    }

    fn systemctl(args: &[&str]) -> Result<()> {
        let st = Command::new("systemctl")
            .arg("--user")
            .args(args)
            .status()
            .context("running systemctl (is systemd available?)")?;
        if !st.success() {
            bail!("systemctl --user {} failed", args.join(" "));
        }
        Ok(())
    }

    pub fn control(action: Action, exe: &Path, config: &Path) -> Result<()> {
        let unit = unit_path()?;
        match action {
            Action::Install => {
                let text = format!(
                    "[Unit]\nDescription=filesort: sorts media from a watch folder into your library\nAfter=network-online.target\n\n\
                     [Service]\nExecStart=\"{}\" --config \"{}\"\nRestart=on-failure\nRestartSec=30\n\n\
                     [Install]\nWantedBy=default.target\n",
                    exe.display(),
                    config.display()
                );
                std::fs::create_dir_all(unit.parent().unwrap())?;
                std::fs::write(&unit, text)?;
                systemctl(&["daemon-reload"])?;
                systemctl(&["enable", "--now", SERVICE_NAME])?;
                println!("installed and started {}", unit.display());
                println!("Logs: journalctl --user -u {SERVICE_NAME} -f");
                println!(
                    "To keep it running while you're logged out: loginctl enable-linger $USER"
                );
            }
            Action::Uninstall => {
                let _ = systemctl(&["disable", "--now", SERVICE_NAME]);
                if unit.exists() {
                    std::fs::remove_file(&unit)?;
                }
                systemctl(&["daemon-reload"])?;
                println!("removed {}", unit.display());
            }
            Action::Start => systemctl(&["start", SERVICE_NAME])?,
            Action::Stop => systemctl(&["stop", SERVICE_NAME])?,
            Action::Status => {
                let _ = Command::new("systemctl")
                    .args(["--user", "status", "--no-pager", SERVICE_NAME])
                    .status();
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------- macOS (launchd)
#[cfg(target_os = "macos")]
mod imp {
    use super::*;
    use anyhow::bail;
    use std::process::Command;

    fn plist_path() -> Result<PathBuf> {
        Ok(dirs::home_dir()
            .context("no home folder")?
            .join("Library/LaunchAgents")
            .join(format!("{LAUNCHD_LABEL}.plist")))
    }

    fn domain() -> String {
        // SAFETY: getuid has no preconditions.
        format!("gui/{}", unsafe { libc_getuid() })
    }

    extern "C" {
        #[link_name = "getuid"]
        fn libc_getuid() -> u32;
    }

    fn launchctl(args: &[&str]) -> Result<()> {
        let st = Command::new("launchctl")
            .args(args)
            .status()
            .context("running launchctl")?;
        if !st.success() {
            bail!("launchctl {} failed", args.join(" "));
        }
        Ok(())
    }

    fn xml(s: &str) -> String {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    }

    pub fn control(action: Action, exe: &Path, config: &Path) -> Result<()> {
        let plist = plist_path()?;
        let target = format!("{}/{LAUNCHD_LABEL}", domain());
        match action {
            Action::Install => {
                let log = config.with_file_name("filesort.log");
                let text = format!(
                    r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{LAUNCHD_LABEL}</string>
  <key>ProgramArguments</key>
  <array><string>{}</string><string>--config</string><string>{}</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>
  <key>StandardOutPath</key><string>{}</string>
  <key>StandardErrorPath</key><string>{}</string>
</dict>
</plist>
"#,
                    xml(&exe.display().to_string()),
                    xml(&config.display().to_string()),
                    xml(&log.display().to_string()),
                    xml(&log.display().to_string()),
                );
                std::fs::create_dir_all(plist.parent().unwrap())?;
                std::fs::write(&plist, text)?;
                let _ = launchctl(&["bootout", &target]);
                launchctl(&["bootstrap", &domain(), &plist.display().to_string()])?;
                println!("installed and started {}", plist.display());
                println!("Logs: {}", log.display());
            }
            Action::Uninstall => {
                let _ = launchctl(&["bootout", &target]);
                if plist.exists() {
                    std::fs::remove_file(&plist)?;
                }
                println!("removed {}", plist.display());
            }
            Action::Start => launchctl(&["kickstart", "-k", &target])?,
            Action::Stop => launchctl(&["kill", "TERM", &target])?,
            Action::Status => {
                let _ = Command::new("launchctl").args(["print", &target]).status();
            }
        }
        Ok(())
    }
}

#[cfg(windows)]
pub use imp::run_as_service;
