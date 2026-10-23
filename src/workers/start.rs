//! Validates application worker targets using Loco's CLI parser and start modes.

use std::ffi::OsString;
use std::fmt;

use loco_rs::boot::StartMode;

use super::target::{WorkerTarget, targets_for_start};

/// The workers a start runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum WorkerStart {
    NoWorkers,
    Targets(Vec<WorkerTarget>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StartError(String);

impl fmt::Display for StartError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for StartError {}

/// Reads CLI worker targets through the framework's authoritative command definition.
pub(crate) fn worker_start_from_args<I, T>(args: I) -> Result<WorkerStart, StartError>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let args: Vec<OsString> = args.into_iter().map(Into::into).collect();
    // Hooks also run in integration binaries, whose arguments are not Loco commands.
    if !args.iter().skip(1).any(|arg| arg == "start" || arg == "s") {
        return Ok(WorkerStart::NoWorkers);
    }
    loco_rs::cli::start_mode_from_args(args)
        .map_err(|error| {
            StartError(format!(
                "could not read the worker targets from the command line: {error}"
            ))
        })?
        .as_ref()
        .map_or(Ok(WorkerStart::NoWorkers), worker_start_from_mode)
}

/// Validates target selection for both CLI and programmatic boots.
pub(crate) fn worker_start_from_mode(mode: &StartMode) -> Result<WorkerStart, StartError> {
    match mode {
        StartMode::WorkerOnly { tags } | StartMode::WorkerAndScheduler { tags } => {
            targets_for_start(tags)
                .map(WorkerStart::Targets)
                .map_err(|error| StartError(error.to_string()))
        }
        StartMode::ServerAndWorker | StartMode::All => Err(StartError(
            "`--all` and `--server-and-worker` start workers that cannot be given a worker target, so they would take none of Yorishiro's jobs; run the workers as a separate process with `start --worker=worker-target:shared`".into(),
        )),
        StartMode::ServerOnly | StartMode::ServerAndScheduler => Ok(WorkerStart::NoWorkers),
    }
}
