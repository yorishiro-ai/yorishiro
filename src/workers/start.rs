//! What a `yorishiro start` command line asks of the workers it starts.
//!
//! `loco_rs::cli::main` parses the command line itself and calls `create_app` without going through `Hooks::boot`, so the only hooks a `start` passes through are `load_config`, `after_context`, `before_run` and `connect_workers`, and none of them is told which worker tags were asked for.
//! The tags are therefore read from the command line again, in `load_config`, before anything is built.
//!
//! loco-rs keeps `Cli` and `Commands` private (`cli.rs`), so [`Cli`] below mirrors the part of them that decides which workers a start runs, with the same attributes as loco-rs 1.2.0 (`cli.rs`, `struct Cli` and `Commands::Start`).
//! clap reads the arguments, so `--worker=a,b`, `--worker a,b`, `-w a,b`, `-w=a,b`, a repeated flag and a bundle of short flags all mean what they mean to loco-rs.
//! By the time `load_config` runs, loco-rs has already parsed the same arguments with its own definition and accepted them, so the mirror only has to agree with it, not to reject anything it would not.
//! Every other subcommand is read as an external one and ignored.

use std::ffi::OsString;
use std::fmt;

use clap::{ArgAction, ArgGroup, Parser, Subcommand};
use loco_rs::boot::StartMode;

use super::target::{WorkerTarget, targets_for_start};

/// Mirror of loco-rs 1.2.0's `Cli` (`cli.rs`, lines 57-67), without the help and version flags.
#[derive(Parser)]
#[command(disable_help_flag = true, disable_version_flag = true)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    #[arg(short, long, global = true)]
    environment: Option<String>,
}

/// Mirror of loco-rs 1.2.0's `Commands::Start` (`cli.rs`, lines 71-96); every other subcommand is [`Commands::Other`].
// The other flags still need fields so clap parses them like loco-rs.
#[derive(Subcommand)]
enum Commands {
    #[command(group(ArgGroup::new("start_mode").args(&["worker", "server_and_worker", "all"])))]
    #[clap(alias("s"))]
    Start {
        #[arg(short, long, action, value_delimiter = ',', num_args = 0.., conflicts_with_all = &["server_and_worker", "all"])]
        worker: Option<Vec<String>>,
        #[arg(short, long, action, conflicts_with_all = &["worker", "all"])]
        server_and_worker: bool,
        #[arg(short, long, action, conflicts_with_all = &["worker", "server_and_worker"])]
        all: bool,
        #[arg(long, action, conflicts_with = "all")]
        scheduler: bool,
        #[arg(short, long, action)]
        binding: Option<String>,
        #[arg(short, long, action)]
        port: Option<i32>,
        #[arg(short, long, action = ArgAction::SetTrue)]
        no_banner: bool,
    },
    #[command(external_subcommand)]
    Other(Vec<OsString>),
}

/// The workers a start runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum WorkerStart {
    /// The command starts no worker: it is another command, or a server (and scheduler) only.
    NoWorkers,
    /// The command starts workers for these targets, each once, in the order given.
    Targets(Vec<WorkerTarget>),
}

/// A start that asks for workers without saying which targets they serve.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StartError(String);

impl fmt::Display for StartError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for StartError {}

fn from_flags(
    worker: Option<Vec<String>>,
    server_and_worker: bool,
    all: bool,
) -> Result<WorkerStart, StartError> {
    // Loco gives these modes no tags, and Yorishiro tags every job it enqueues, so such a worker would take none of them.
    if server_and_worker || all {
        return Err(StartError(
            "`--all` and `--server-and-worker` start workers that cannot be given a worker target, so they would take none of Yorishiro's jobs; run the workers as a separate process with `start --worker=worker-target:shared`".into(),
        ));
    }
    match worker {
        None => Ok(WorkerStart::NoWorkers),
        Some(tags) => targets_for_start(&tags)
            .map(WorkerStart::Targets)
            .map_err(|error| StartError(error.to_string())),
    }
}

/// Reads the workers a command line starts.
///
/// `args` is the whole command line, program name first.
/// A command line with no `start` (or `s`) in it is not read at all.
///
/// # Errors
/// Returns an error when the command starts workers and names no target, names one that is not valid, or uses `--all` or `--server-and-worker`; or when the arguments cannot be read, which would mean this mirror of loco-rs's definition has drifted from it.
pub(crate) fn worker_start_from_args<I, T>(args: I) -> Result<WorkerStart, StartError>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    let args: Vec<OsString> = args.into_iter().map(Into::into).collect();
    if !args.iter().skip(1).any(|arg| arg == "start" || arg == "s") {
        return Ok(WorkerStart::NoWorkers);
    }
    let cli = Cli::try_parse_from(&args).map_err(|error| {
        StartError(format!(
            "could not read the worker targets from the command line: {error}"
        ))
    })?;
    let _ = cli.environment;
    match cli.command {
        Commands::Start {
            worker,
            server_and_worker,
            all,
            scheduler,
            binding,
            port,
            no_banner,
        } => {
            let _ = (scheduler, binding, port, no_banner);
            from_flags(worker, server_and_worker, all)
        }
        Commands::Other(args) => {
            let _ = args;
            Ok(WorkerStart::NoWorkers)
        }
    }
}

/// Reads the workers a [`StartMode`] starts, for a caller that boots the application without a command line.
///
/// # Errors
/// Returns an error in the same cases as [`worker_start_from_args`].
pub(crate) fn worker_start_from_mode(mode: &StartMode) -> Result<WorkerStart, StartError> {
    match mode {
        StartMode::WorkerOnly { tags } | StartMode::WorkerAndScheduler { tags } => {
            from_flags(Some(tags.clone()), false, false)
        }
        StartMode::ServerAndWorker | StartMode::All => from_flags(None, true, false),
        StartMode::ServerOnly | StartMode::ServerAndScheduler => Ok(WorkerStart::NoWorkers),
    }
}
