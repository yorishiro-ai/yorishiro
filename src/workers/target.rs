//! Which workers a queued job is for.
//!
//! A job names exactly one target, and a worker process names the targets it serves when it starts (`yorishiro start --worker=worker-target:shared`); a start that names none is refused.
//! The target is carried as a single composite tag, `worker-target:<name>`.
//! Every queue provider matches a worker's tags against a job's tags with OR, so a class tag and a separate owner tag would match on either one; one tag that already says both cannot.
//!
//! | Target | Class | Concurrency key |
//! |---|---|---|
//! | `shared` | `Shared` | `shared:shared` |
//! | `official` | `Official` | `official:official` |
//! | `tenant:<tenant-id>` | `TenantPrivate` | `tenant_private:tenant:<tenant-id>` |
//!
//! The class is still recorded on the job's lifecycle row, but it no longer takes part in choosing a worker.
//! The concurrency key is the same name used for the tag, so the set of workers a job can reach and the set of jobs counted against its limit are always the same set.

use std::fmt;

use sea_orm::ConnectionTrait;
use uuid::Uuid;

use crate::error::YorishiroError;
use crate::workers::embedding_sync::WorkerClass;

/// What every worker tag begins with.
const TAG_PREFIX: &str = "worker-target:";

/// The one place a job can run, as a worker process names it at start.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WorkerTarget {
    /// Compute any deployment shares.
    /// This is what the packaged service and the compose file serve.
    Shared,
    /// Compute this deployment operates itself.
    Official,
    /// Compute reserved for one tenant's own workspaces.
    Tenant(Uuid),
}

/// A worker tag that names no target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidWorkerTarget {
    tag: String,
    reason: &'static str,
}

impl fmt::Display for InvalidWorkerTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        const USE: &str = "use `worker-target:shared`, `worker-target:official`, or `worker-target:tenant:<tenant-id>` with the tenant id in lowercase hyphenated form";
        if self.tag.is_empty() {
            write!(formatter, "{}; {USE}", self.reason)
        } else {
            write!(
                formatter,
                "invalid worker target {:?}: {}; {USE}",
                self.tag, self.reason
            )
        }
    }
}

impl std::error::Error for InvalidWorkerTarget {}

impl WorkerTarget {
    /// The class a job for this target is recorded under.
    #[must_use]
    pub const fn class(self) -> WorkerClass {
        match self {
            Self::Shared => WorkerClass::Shared,
            Self::Official => WorkerClass::Official,
            Self::Tenant(_) => WorkerClass::TenantPrivate,
        }
    }

    /// The target's name without the tag prefix: `shared`, `official`, or `tenant:<tenant-id>`.
    #[must_use]
    pub(crate) fn name(self) -> String {
        match self {
            Self::Shared => "shared".to_owned(),
            Self::Official => "official".to_owned(),
            Self::Tenant(tenant_id) => format!("tenant:{}", tenant_id.hyphenated()),
        }
    }

    /// The tag a job for this target carries and a worker for it is started with.
    #[must_use]
    pub fn tag(self) -> String {
        format!("{TAG_PREFIX}{}", self.name())
    }

    /// The named queue this target's jobs use on a provider that honours one (Redis).
    ///
    /// A Redis worker filters tags client-side over the first 1000 entries of a queue, so one queue per target keeps a backlog for one target from hiding the jobs of another.
    #[must_use]
    pub fn queue(self) -> String {
        self.tag()
    }

    /// The key the concurrency limit counts running jobs under.
    #[must_use]
    pub fn concurrency_key(self) -> String {
        format!("{}:{}", self.class().as_db_str(), self.name())
    }

    /// Reads a worker tag back into the target it names.
    ///
    /// The tenant id must already be in the form `Uuid::hyphenated` prints (lowercase, with hyphens).
    /// SQLite compares tags with `LIKE`, which ignores case, so two spellings of one id would be one target there and two on the other providers.
    ///
    /// # Errors
    /// Returns an error when `tag` does not start with `worker-target:`, names an unknown target, or carries a tenant id that is not in canonical form.
    pub fn parse_tag(tag: &str) -> Result<Self, InvalidWorkerTarget> {
        let invalid = |reason| InvalidWorkerTarget {
            tag: tag.to_owned(),
            reason,
        };
        let name = tag
            .strip_prefix(TAG_PREFIX)
            .ok_or_else(|| invalid("a worker tag must start with `worker-target:`"))?;
        match name {
            "shared" => Ok(Self::Shared),
            "official" => Ok(Self::Official),
            _ => {
                let id = name.strip_prefix("tenant:").ok_or_else(|| {
                    invalid("the target is not one of shared, official, or tenant")
                })?;
                let tenant_id =
                    Uuid::parse_str(id).map_err(|_| invalid("the tenant id is not a UUID"))?;
                if tenant_id.hyphenated().to_string() != id {
                    return Err(invalid("the tenant id is not in lowercase hyphenated form"));
                }
                Ok(Self::Tenant(tenant_id))
            }
        }
    }

    /// The target a job of `class` for `workspace_id` is delivered to.
    ///
    /// Only a tenant's own compute needs the workspace: it names the tenant that owns it.
    ///
    /// # Errors
    /// Returns an error when a `TenantPrivate` job's workspace no longer exists or cannot be read.
    pub(crate) async fn for_job(
        conn: &impl ConnectionTrait,
        class: WorkerClass,
        workspace_id: Uuid,
    ) -> Result<Self, YorishiroError> {
        Ok(match class {
            WorkerClass::Shared => Self::Shared,
            WorkerClass::Official => Self::Official,
            WorkerClass::TenantPrivate => Self::Tenant(
                crate::models::workspace_workspaces::get_workspace_tenant(conn, workspace_id)
                    .await?,
            ),
        })
    }
}

/// Checks the tags of a worker start request and returns the targets they name.
///
/// A start has to name its targets.
/// A worker that names none would take only untagged jobs, and Yorishiro tags every job it enqueues, so it would sit idle; a start that says so is refused rather than quietly serving a default.
/// Tags are returned in the order given, each once.
///
/// # Errors
/// Returns an error when no tag is given, or for the first tag that names no target.
pub fn targets_for_start(tags: &[String]) -> Result<Vec<WorkerTarget>, InvalidWorkerTarget> {
    if tags.is_empty() {
        return Err(InvalidWorkerTarget {
            tag: String::new(),
            reason: "a worker start names no target",
        });
    }
    let mut targets = Vec::with_capacity(tags.len());
    for tag in tags {
        let target = WorkerTarget::parse_tag(tag)?;
        if !targets.contains(&target) {
            targets.push(target);
        }
    }
    Ok(targets)
}
