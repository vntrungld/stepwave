//! `stepwave.exe`: `run` (the long-running app, self-supervised — see `stepwave_winapp::supervise`),
//! `install`/`uninstall` (logon task) and the control commands shared with the Linux daemon.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use stepwave_host::protocol::{ModeArg, Request, Response};
use stepwave_winapp::pipe::{self, PIPE_NAME};

#[derive(Parser)]
#[command(
    name = "stepwave",
    version,
    about = "stepwave footstep enhancer for Windows"
)]
struct Cli {
    /// Control pipe name (default: stepwave)
    #[arg(long, global = true, default_value = PIPE_NAME)]
    pipe: String,
    /// Print the raw JSON reply
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run stepwave (normally started at logon by the task `stepwave install` creates)
    Run {
        /// Profiles directory (default: %LOCALAPPDATA%\stepwave\profiles)
        #[arg(long)]
        profiles: Option<PathBuf>,
        /// Initial mode
        #[arg(long, value_enum, default_value = "auto")]
        mode: CliMode,
        /// Internal: this process IS the app (spawned by the supervisor); do not supervise
        /// again. Not for interactive use.
        #[arg(long, hide = true)]
        supervised: bool,
    },
    /// Start stepwave at logon (per-user Task Scheduler task; no admin rights)
    Install {
        /// Extra arguments for `run`, e.g. "--mode eq"
        #[arg(long, default_value = "")]
        run_args: String,
    },
    /// Remove the logon task
    Uninstall,
    /// Show what stepwave is doing
    Status,
    /// Enable processing
    On,
    /// Bypass processing (latency unchanged, for fair A/B)
    Off,
    /// Flip on/off (bind this to a hotkey)
    Toggle,
    /// Processing mode
    Mode {
        #[arg(value_enum)]
        mode: CliMode,
    },
    /// Override the profile's strength (dB, 0..24) until reload/restart
    Strength { db: f32 },
    /// Pin a profile, or `--auto` to unpin
    Profile {
        #[arg(required_unless_present = "auto")]
        id: Option<String>,
        #[arg(long, conflicts_with = "id")]
        auto: bool,
    },
    /// Re-read profiles and models from disk
    Reload,
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum CliMode {
    Auto,
    Model,
    Eq,
    Bypass,
}

impl From<CliMode> for ModeArg {
    fn from(m: CliMode) -> Self {
        match m {
            CliMode::Auto => ModeArg::Auto,
            CliMode::Model => ModeArg::Model,
            CliMode::Eq => ModeArg::Eq,
            CliMode::Bypass => ModeArg::Bypass,
        }
    }
}

fn main() -> ExitCode {
    // Captured before `Cli::parse()` consumes nothing (clap borrows argv, it doesn't take it):
    // the supervisor re-spawns this exact argv (plus `--supervised`) rather than reconstructing
    // it from the parsed options, so any flag it doesn't know about still passes through.
    let argv: Vec<OsString> = std::env::args_os().skip(1).collect();
    let cli = Cli::parse();
    let request = match cli.command {
        Command::Run {
            profiles,
            mode,
            supervised,
        } => return run(profiles, mode, cli.pipe, supervised, argv),
        Command::Install { run_args } => return report(install(&run_args)),
        Command::Uninstall => return report(uninstall()),
        Command::Status => Request::Status,
        Command::On => Request::On,
        Command::Off => Request::Off,
        Command::Toggle => Request::Toggle,
        Command::Mode { mode } => Request::Mode(mode.into()),
        Command::Strength { db } => Request::Strength(db),
        Command::Profile { id, .. } => Request::Profile(id),
        Command::Reload => Request::Reload,
    };
    match pipe::request(&cli.pipe, &request) {
        Ok(resp) => print_response(&resp, cli.json),
        Err(e) => {
            eprintln!("stepwave: {e}");
            ExitCode::FAILURE
        }
    }
}

fn report(result: anyhow::Result<()>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("stepwave: {e:#}");
            ExitCode::FAILURE
        }
    }
}

/// `%LOCALAPPDATA%\stepwave\profiles`.
fn default_profiles_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("stepwave")
        .join("profiles")
}

#[cfg(windows)]
fn run(
    profiles: Option<PathBuf>,
    mode: CliMode,
    pipe: String,
    supervised: bool,
    argv: Vec<OsString>,
) -> ExitCode {
    if !supervised {
        // Task Scheduler's restart-on-failure only covers a failed launch, so `run` supervises
        // itself for crashes after that (see `supervise`): re-exec the same argv with
        // `--supervised` added, and restart the child on an unexpected exit.
        return stepwave_winapp::supervise::run_supervised(&argv);
    }
    let opts = stepwave_winapp::app::Options {
        profiles_dir: profiles.unwrap_or_else(default_profiles_dir),
        mode: mode.into(),
        pipe,
    };
    report_run(stepwave_winapp::app::run(opts))
}

#[cfg(not(windows))]
fn run(
    profiles: Option<PathBuf>,
    _mode: CliMode,
    _pipe: String,
    _supervised: bool,
    _argv: Vec<OsString>,
) -> ExitCode {
    let dir = profiles.unwrap_or_else(default_profiles_dir);
    eprintln!(
        "stepwave: `run` needs Windows (WASAPI); on Linux use the stepwave daemon ({})",
        dir.display()
    );
    ExitCode::FAILURE
}

/// Like `report`, but gives `app::run`'s "another instance is running" failure its own exit
/// code so the supervisor recognises it and does not retry against the same live instance.
#[cfg(windows)]
fn report_run(result: anyhow::Result<()>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("stepwave: {e:#}");
            if is_addr_in_use(&e) {
                ExitCode::from(stepwave_winapp::supervise::ADDR_IN_USE_CODE as u8)
            } else {
                ExitCode::FAILURE
            }
        }
    }
}

#[cfg(windows)]
fn is_addr_in_use(e: &anyhow::Error) -> bool {
    e.chain()
        .filter_map(|c| c.downcast_ref::<std::io::Error>())
        .any(|io| io.kind() == std::io::ErrorKind::AddrInUse)
}

#[cfg(windows)]
fn install(run_args: &str) -> anyhow::Result<()> {
    stepwave_winapp::install::install(run_args)
}

#[cfg(not(windows))]
fn install(_run_args: &str) -> anyhow::Result<()> {
    anyhow::bail!("`install` creates a Windows Task Scheduler task and needs Windows")
}

#[cfg(windows)]
fn uninstall() -> anyhow::Result<()> {
    stepwave_winapp::install::uninstall()
}

#[cfg(not(windows))]
fn uninstall() -> anyhow::Result<()> {
    anyhow::bail!("`uninstall` needs Windows")
}

fn print_response(resp: &Response, json: bool) -> ExitCode {
    if json {
        print!("{}", resp.to_line());
    } else if let Some(err) = &resp.error {
        eprintln!("stepwave: {err}");
    } else if let Some(status) = &resp.status {
        print!("{}", status.to_text());
    }
    if resp.ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
