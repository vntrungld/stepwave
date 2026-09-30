//! `stepwave`: the Linux daemon and its control commands.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use stepwave_daemon::control;
use stepwave_daemon::daemon::{self, Options};
use stepwave_daemon::protocol::{ModeArg, Request, Response};

#[derive(Parser)]
#[command(
    name = "stepwave",
    version,
    about = "stepwave footstep enhancer for PipeWire"
)]
struct Cli {
    /// Control socket (default: $XDG_RUNTIME_DIR/stepwave.sock)
    #[arg(long, global = true)]
    socket: Option<PathBuf>,
    /// Print the daemon's raw JSON reply
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the daemon (normally started by systemd)
    Daemon {
        /// Profiles directory (default: ~/.config/stepwave/profiles)
        #[arg(long)]
        profiles: Option<PathBuf>,
        /// Initial mode
        #[arg(long, value_enum, default_value = "auto")]
        mode: CliMode,
    },
    /// Show what the daemon is doing
    Status,
    /// Enable processing
    On,
    /// Bypass processing (latency unchanged, for fair A/B)
    Off,
    /// Flip on/off (bind this to a global shortcut)
    Toggle,
    /// Processing mode
    Mode {
        #[arg(value_enum)]
        mode: CliMode,
    },
    /// Override the active profile's strength (dB, 0..24) until reload/restart
    Strength { db: f32 },
    /// Pin a profile, or `--auto` to follow the detected game again
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
    let cli = Cli::parse();
    let socket = match cli
        .socket
        .clone()
        .map(Ok)
        .unwrap_or_else(control::default_socket_path)
    {
        Ok(s) => s,
        Err(e) => {
            eprintln!("stepwave: {e}");
            return ExitCode::FAILURE;
        }
    };
    let request = match cli.command {
        Command::Daemon { profiles, mode } => {
            let profiles_dir = profiles.unwrap_or_else(default_profiles_dir);
            let opts = Options {
                profiles_dir,
                mode: mode.into(),
                socket,
            };
            return match daemon::run(opts) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("stepwave: {e:#}");
                    ExitCode::FAILURE
                }
            };
        }
        Command::Status => Request::Status,
        Command::On => Request::On,
        Command::Off => Request::Off,
        Command::Toggle => Request::Toggle,
        Command::Mode { mode } => Request::Mode(mode.into()),
        Command::Strength { db } => Request::Strength(db),
        Command::Profile { id, .. } => Request::Profile(id),
        Command::Reload => Request::Reload,
    };
    match control::request(&socket, &request) {
        Ok(resp) => print_response(&resp, cli.json),
        Err(e) => {
            eprintln!("stepwave: {e}");
            ExitCode::FAILURE
        }
    }
}

fn default_profiles_dir() -> PathBuf {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    config.join("stepwave/profiles")
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
