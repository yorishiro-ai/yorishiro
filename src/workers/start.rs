//! Validates application worker targets after Loco has parsed the command line.

use std::ffi::OsString;
use std::fmt;

use loco_rs::boot::StartMode;

use super::target::{WorkerTarget, targets_for_start};

/// The workers a start runs.
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum WorkerStart {
    NoWorkers,
    Targets(Vec<WorkerTarget>),
    Jobs(Vec<WorkerTarget>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StartError(String);

impl fmt::Display for StartError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for StartError {}

/// Extracts application routing tags; Loco remains responsible for CLI syntax validation.
pub(crate) fn worker_start_from_args<I, T>(args: I) -> Result<WorkerStart, StartError>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let args: Vec<OsString> = args.into_iter().map(Into::into).collect();
    // Hooks also run in integration binaries, whose arguments are not Loco commands.
    let Some(start) = args.iter().position(|arg| arg == "start" || arg == "s") else {
        return Ok(WorkerStart::NoWorkers);
    };
    let mut tags = None;
    let mut values = false;
    for arg in &args[start + 1..] {
        let arg = arg.to_string_lossy();
        if matches!(arg.as_ref(), "--all" | "-a" | "--server-and-worker" | "-s") {
            return worker_start_from_mode(&StartMode::All);
        }
        if matches!(arg.as_ref(), "--worker" | "-w") {
            tags.get_or_insert_with(Vec::new);
            values = true;
        } else if let Some(value) = arg
            .strip_prefix("--worker=")
            .or_else(|| arg.strip_prefix("-w="))
        {
            tags.get_or_insert_with(Vec::new)
                .extend(value.split(',').map(str::to_owned));
            values = true;
        } else if arg.starts_with('-') {
            values = false;
        } else if values {
            tags.get_or_insert_with(Vec::new)
                .extend(arg.split(',').map(str::to_owned));
        }
    }
    tags.map_or(Ok(WorkerStart::NoWorkers), |tags| {
        worker_start_from_mode(&StartMode::WorkerOnly { tags })
    })
}

/// Validates target selection for both CLI and programmatic boots.
pub(crate) fn worker_start_from_mode(mode: &StartMode) -> Result<WorkerStart, StartError> {
    match mode {
        StartMode::WorkerOnly { tags } | StartMode::WorkerAndScheduler { tags } => {
            if tags.first().is_some_and(|tag| tag.starts_with("job-target:")) {
                let targets = tags.iter().map(|tag| WorkerTarget::parse_job_tag(tag))
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|error| StartError(error.to_string()))?;
                return Ok(WorkerStart::Jobs(targets));
            }
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
