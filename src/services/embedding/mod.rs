//! Embedding provider abstraction.

pub mod local;
mod model_fetch;
pub mod openai;
pub mod sync;

pub use openai::{OpenAiCompatibleConfig, OpenAiCompatibleProvider};

use std::sync::Arc;

use async_trait::async_trait;
use uuid::Uuid;

use crate::error::YorishiroError;

/// What a piece of text is being embedded for.
///
/// Asymmetric models expect a search query to carry an instruction prefix that a stored document must not have.
/// Embedding both the same way costs nothing visible: the vectors are the right shape and normalize, the results are just worse.
/// Providers that need no such distinction ignore this.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbedKind {
    /// Text a user or agent is searching with.
    Query,
    /// Text being stored and later searched for.
    Document,
}

/// Provider that generates embedding vectors.
/// The `content_entities.embedding` column is dimensionless (`vector`), so any model works.
/// All vectors in a deployment must share the same dimension count.
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    fn dimensions(&self) -> usize;

    /// Identifies the model this provider embeds with, for stamping onto a workspace at creation.
    ///
    /// Each implementation names itself rather than a caller inferring it from configuration: `YORISHIRO_EMBEDDING_MODEL` only ever holds the OpenAI-compatible provider's model, so a caller that read it regardless of which provider it actually got would record `"unconfigured"` for every local-provider workspace, unable to tell "no embeddings configured" apart from "embeddings configured, just not through that variable".
    fn model_name(&self) -> String;

    /// How many tokens `text` costs this provider, for quota purposes.
    ///
    /// The default is a byte-length estimate, and deliberately so: a provider without a tokenizer in the process (an external API, where the model runs elsewhere) cannot count exactly, and loading one purely to meter would mean shipping a tokenizer to a deployment that chose not to run embeddings locally.
    ///
    /// Four bytes per token is the usual English rule of thumb and overestimates Japanese text, which suits a quota: overcharging throttles a heavy caller early, while undercharging lets it past the limit it was supposed to hit.
    fn count_tokens(&self, text: &str) -> u32 {
        u32::try_from(text.len().div_ceil(4)).unwrap_or(u32::MAX)
    }

    /// Must return vectors in the same order and count as the input.
    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, YorishiroError>;

    /// Embeds `text` knowing what it is for.
    ///
    /// The default ignores `kind` and delegates to [`Self::embed`], which is correct for every symmetric model.
    /// A provider whose model treats queries and documents differently overrides this.
    async fn embed_as(&self, kind: EmbedKind, text: &str) -> Result<Vec<f32>, YorishiroError> {
        let _ = kind;
        self.embed(text).await
    }

    /// Default implementation delegates to `embed_batch`.
    async fn embed(&self, text: &str) -> Result<Vec<f32>, YorishiroError> {
        let batch = self.embed_batch(&[text]).await?;
        batch.into_iter().next().ok_or_else(|| {
            YorishiroError::Internal(anyhow::anyhow!(
                "embedding provider returned no vectors for a single input"
            ))
        })
    }
}

/// Resolves a workspace's own embedding provider, if it has one.
///
/// A seam: a deployment can let a workspace point at a different embedding backend than the deployment default (its own local model, a different OpenAI-compatible endpoint) without touching the callers that resolve a provider.
/// [`DefaultEmbeddingResolver`] is the behaviour of every deployment that does not replace it: every workspace uses the deployment-wide provider.
///
/// `conn` is `ctx.db` (Loco's own `DatabaseConnection`), not the RLS-scoped tenant pool: a per-workspace assignment is deployment configuration, read the same way `identity_workspace_llm_keys` is, not tenant content.
/// This is why `conn` takes a `sea_orm::DatabaseConnection` rather than `DbHandle`: `DbHandle` does not exist on SQLite (see `Hooks::after_context`), and this seam must work on both backends, unlike `Authenticator`, which is a PostgreSQL/RLS-only concept by design.
///
/// Returns `Ok(None)` when the workspace has no assignment of its own, so the caller falls back to the deployment default already held in `shared_store` rather than this seam constructing it: building the fallback (a local model load can be hundreds of megabytes) is a cost only worth paying once, not on every call whether or not a workspace override exists.
/// No caching: this runs once per call, same as `identity_workspace_llm_keys::get`.
/// Acceptable for the same reason it is there: a metadata read, not the slow work.
#[async_trait]
pub trait WorkspaceEmbeddingResolver: Send + Sync {
    async fn resolve(
        &self,
        conn: &sea_orm::DatabaseConnection,
        workspace_id: Uuid,
    ) -> Result<Option<Arc<dyn EmbeddingProvider>>, YorishiroError>;
}

/// This crate's own rule: no workspace has its own provider, so every caller falls back to the deployment default.
pub struct DefaultEmbeddingResolver;

#[async_trait]
impl WorkspaceEmbeddingResolver for DefaultEmbeddingResolver {
    async fn resolve(
        &self,
        _conn: &sea_orm::DatabaseConnection,
        _workspace_id: Uuid,
    ) -> Result<Option<Arc<dyn EmbeddingProvider>>, YorishiroError> {
        Ok(None)
    }
}

/// The resolver a deployment gets when it does not choose one.
pub fn default_embedding_resolver() -> Arc<dyn WorkspaceEmbeddingResolver> {
    Arc::new(DefaultEmbeddingResolver)
}

/// A provider that satisfies the dimension count but errors on every actual call.
/// Stands in when no embedding backend could be configured, so boot succeeds; search/recall simply error if invoked.
pub struct UnconfiguredEmbeddingProvider {
    dimensions: usize,
    /// What this particular deployment should set to get a working provider.
    ///
    /// Carried per instance rather than hardcoded in [`Self::embed_batch`], because the two ways of arriving here need opposite advice: an unset `YORISHIRO_EMBEDDING_BASE_URL` wants the OpenAI-compatible variables, while `YORISHIRO_EMBEDDING_PROVIDER=local` with nowhere to fetch to wants the local model path variables.
    /// The boot log says the same thing, but it scrolls away, and this error is what an operator keeps hitting afterwards.
    remedy: &'static str,
}

#[async_trait]
impl EmbeddingProvider for UnconfiguredEmbeddingProvider {
    fn dimensions(&self) -> usize {
        self.dimensions
    }

    fn model_name(&self) -> String {
        "unconfigured".into()
    }

    async fn embed_batch(&self, _texts: &[&str]) -> Result<Vec<Vec<f32>>, YorishiroError> {
        Err(YorishiroError::ProviderUnreachable {
            url: String::new(),
            message: format!("no embedding provider is configured: {}", self.remedy),
        })
    }
}

/// Builds the embedding provider from environment variables.
///
/// `YORISHIRO_EMBEDDING_PROVIDER=local` selects the local in-process provider (needs `YORISHIRO_LOCAL_MODEL_PATH`/`YORISHIRO_LOCAL_TOKENIZER_PATH`, no external service).
/// Otherwise, `YORISHIRO_EMBEDDING_BASE_URL`/`YORISHIRO_EMBEDDING_MODEL` select the OpenAI-compatible provider (LM Studio, Ollama, vLLM, or real OpenAI); when either is unset, boot proceeds with [`UnconfiguredEmbeddingProvider`] rather than failing.
/// `YORISHIRO_EMBEDDING_DIMENSIONS` defaults to 768.
pub async fn build_embedding_provider() -> anyhow::Result<std::sync::Arc<dyn EmbeddingProvider>> {
    let dimensions: usize = std::env::var("YORISHIRO_EMBEDDING_DIMENSIONS")
        .unwrap_or_else(|_| "768".into())
        .parse()?;

    if std::env::var("YORISHIRO_EMBEDDING_PROVIDER").as_deref() == Ok("local") {
        return build_local_provider(dimensions).await;
    }

    let base_url = std::env::var("YORISHIRO_EMBEDDING_BASE_URL").ok();
    let model = std::env::var("YORISHIRO_EMBEDDING_MODEL").ok();
    let (base_url, model) = match (base_url, model) {
        (Some(base_url), Some(model)) => (base_url, model),
        _ => {
            tracing::info!(
                "no embedding provider configured (YORISHIRO_EMBEDDING_BASE_URL/YORISHIRO_EMBEDDING_MODEL unset)"
            );
            return Ok(std::sync::Arc::new(UnconfiguredEmbeddingProvider {
                dimensions,
                remedy: "set YORISHIRO_EMBEDDING_BASE_URL and YORISHIRO_EMBEDDING_MODEL",
            }));
        }
    };

    let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleConfig {
        base_url: base_url.clone(),
        api_key: std::env::var("YORISHIRO_EMBEDDING_API_KEY").unwrap_or_default(),
        model: model.clone(),
        dimensions,
        send_dimensions_param: std::env::var("YORISHIRO_EMBEDDING_SEND_DIMENSIONS_PARAM")
            .map(|v| v == "true")
            .unwrap_or(false),
    });
    tracing::info!(provider = "openai", %base_url, %model, dimensions, "embedding provider configured");
    Ok(std::sync::Arc::new(provider))
}

/// What became of one old `YORISHIRO_ONNX_*` variable.
/// The two outcomes carry genuinely different risk, so [`reject_renamed_onnx_vars`] must not give
/// them the same message: a stale `Renamed` variable can silently start this provider on the
/// wrong model, but a stale `Removed` one cannot, since nothing reads it and there is no
/// alternative behaviour left for it to have selected.
enum OnnxVarFate {
    /// Reading this old name would have changed which model, tokenizer, or truncation this
    /// provider used, so leaving it set risks starting a different model than the deployment
    /// thinks it has.
    Renamed(&'static str),
    /// This variable named a mechanism removed outright ([`Pooling`] having one legal value, or
    /// nomic-embed-text-v1.5 never reading the Qwen3-style instruction this variable rendered).
    /// A stale value here changes nothing: it names no risk, only that the setting is inert.
    Removed,
}

/// The `YORISHIRO_ONNX_*` variables this deployment might still have set from before the local
/// provider moved from `ort`/ONNX to candle.
const RENAMED_ONNX_VARS: [(&str, OnnxVarFate); 5] = [
    (
        "YORISHIRO_ONNX_MODEL_PATH",
        OnnxVarFate::Renamed("YORISHIRO_LOCAL_MODEL_PATH"),
    ),
    (
        "YORISHIRO_ONNX_TOKENIZER_PATH",
        OnnxVarFate::Renamed("YORISHIRO_LOCAL_TOKENIZER_PATH"),
    ),
    (
        "YORISHIRO_ONNX_MAX_SEQUENCE_LENGTH",
        OnnxVarFate::Renamed("YORISHIRO_LOCAL_MAX_SEQUENCE_LENGTH"),
    ),
    ("YORISHIRO_ONNX_POOLING", OnnxVarFate::Removed),
    ("YORISHIRO_ONNX_QUERY_INSTRUCTION", OnnxVarFate::Removed),
];

/// Fails startup when an old `YORISHIRO_ONNX_*` variable is still set.
///
/// A stale `YORISHIRO_ONNX_MODEL_PATH` naming an operator's own model is never read: it is simply
/// invisible to [`resolve_local_paths`], which then runs its normal resolution as if nothing had
/// been configured (using the `models/` default files if both are present, fetching nomic-embed-
/// text-v1.5 if neither is, or erroring on an incomplete pair). A deployment that had a different
/// model configured under the old name would, without this check, silently start writing vectors
/// from a different model into an index built for the one it thinks it still has, with every
/// status staying green. Refusing to boot forces the operator to remove or rename the variable
/// (and confirm the resulting resolution is what they actually want) before that can happen,
/// rather than a log line they could reasonably miss during an otherwise successful upgrade.
///
/// `YORISHIRO_ONNX_POOLING`/`YORISHIRO_ONNX_QUERY_INSTRUCTION` still fail startup too, for
/// consistency (every old name gets the same "stop and clean this up" treatment rather than some
/// silently tolerated), but their message must not claim the wrong-model risk above: neither
/// variable is read by anything, so a stale value changes no behaviour at all. Claiming a risk
/// that does not exist would cost the accurate claim above its credibility on the next reader.
fn reject_renamed_onnx_vars() -> anyhow::Result<()> {
    for (old, fate) in RENAMED_ONNX_VARS {
        // `var_os`, not `var`: `var` returns `Err` both when the variable is unset and when it is
        // set to a non-UTF-8 value, so checking `.is_err()` would let a non-UTF-8
        // `YORISHIRO_ONNX_MODEL_PATH` slip past this guard as though it were absent. `var_os`
        // only cares whether the variable is present, regardless of what it decodes to.
        if std::env::var_os(old).is_none() {
            continue;
        }
        match fate {
            OnnxVarFate::Renamed(new) => anyhow::bail!(
                "{old} is set but no longer read; the local embedding provider now reads {new}. \
                 Remove {old} (or rename it to {new} if it should still apply) before starting: \
                 leaving it set would silently start this provider on a different model than the \
                 one {old} names, which is not safe to detect and correct after the fact once \
                 vectors from the wrong model have been written."
            ),
            OnnxVarFate::Removed => anyhow::bail!(
                "{old} is set but no longer has any effect: this setting was removed rather than \
                 renamed, and nothing reads it. Remove {old} before starting; leaving it set \
                 changes no behaviour, but its presence claims a configuration this deployment no \
                 longer has."
            ),
        }
    }
    Ok(())
}

/// Picks a [`model_fetch::LocalModelDef`] by `YORISHIRO_LOCAL_MODEL`'s value (one of `model_fetch::MODELS`'s `short_id`s), or [`model_fetch::DEFAULT_MODEL`] when unset.
/// An unrecognized value fails startup rather than silently falling back to the default: a typo in this variable is exactly the kind of "this deployment thinks it configured one model but got another" mistake the whole write-time model check exists to catch, and catching the typo at boot is strictly better than catching the resulting stamp mismatch on the first write.
fn resolve_local_model() -> anyhow::Result<&'static model_fetch::LocalModelDef> {
    let Some(requested) = std::env::var_os("YORISHIRO_LOCAL_MODEL") else {
        return Ok(model_fetch::DEFAULT_MODEL);
    };
    let requested = requested
        .into_string()
        .map_err(|_| anyhow::anyhow!("YORISHIRO_LOCAL_MODEL is not valid UTF-8"))?;
    model_fetch::MODELS
        .iter()
        .find(|def| def.short_id == requested)
        .copied()
        .ok_or_else(|| {
            let known: Vec<&str> = model_fetch::MODELS.iter().map(|def| def.short_id).collect();
            anyhow::anyhow!(
                "YORISHIRO_LOCAL_MODEL={requested:?} is not a known local model; valid values are: {}",
                known.join(", ")
            )
        })
}

/// `YORISHIRO_EMBEDDING_PROVIDER=local`'s branch of [`build_embedding_provider`].
/// `YORISHIRO_LOCAL_MODEL` selects which [`model_fetch::LocalModelDef`] to load (default: [`model_fetch::DEFAULT_MODEL`]); see [`resolve_local_model`].
/// `YORISHIRO_LOCAL_MODEL_PATH`/`YORISHIRO_LOCAL_TOKENIZER_PATH` default to `models/<short_id>/model.safetensors`/`models/<short_id>/tokenizer.json` for the selected model; `YORISHIRO_LOCAL_MAX_SEQUENCE_LENGTH` defaults to 512 regardless of which model is selected, unchanged from this provider's behavior before model selection existed.
/// Not the selected model's own upper bound: nomic's own bound is 8192, and silently truncating further out by default the moment a deployment's `YORISHIRO_LOCAL_MODEL` stays unset (still nomic; see [`model_fetch::DEFAULT_MODEL`]) would be a memory/latency change with no corresponding request for one.
/// `def.max_sequence_length` still validates the setting (see [`local::LocalEmbeddingProvider::load`]): 512 already satisfies every current definition's own bound, nomic's and multilingual-e5-base's alike, so this default needs no per-model branch of its own.
/// Setting *either* path variable also turns the automatic fetch off, for both files: those defaults describe where an unset variable points, not a fallback the fetch still applies behind them.
///
/// There is no pooling variable: every model this provider can load is mean-pooled (see `local::LocalEmbeddingProvider`'s own doc comment for why that stays a fact rather than a config surface).
///
/// The model files are fetched on first use when, and only when, neither path variable is set and nothing is at the default path; see [`resolve_local_paths`].
async fn build_local_provider(
    dimensions: usize,
) -> anyhow::Result<std::sync::Arc<dyn EmbeddingProvider>> {
    reject_renamed_onnx_vars()?;
    let def = resolve_local_model()?;
    let max_sequence_length: usize = std::env::var("YORISHIRO_LOCAL_MAX_SEQUENCE_LENGTH")
        .unwrap_or_else(|_| "512".into())
        .parse()?;
    let Some((model_path, tokenizer_path)) = resolve_local_paths(def).await? else {
        return Ok(std::sync::Arc::new(UnconfiguredEmbeddingProvider {
            dimensions,
            // `YORISHIRO_EMBEDDING_PROVIDER=local` is already set, so pointing at the OpenAI-compatible variables here would answer a question this operator did not ask.
            remedy: "YORISHIRO_EMBEDDING_PROVIDER=local, but the model files could not be \
                     located or fetched: set YORISHIRO_LOCAL_MODEL_PATH and \
                     YORISHIRO_LOCAL_TOKENIZER_PATH",
        }));
    };
    let model_path = model_path.display().to_string();
    let tokenizer_path = tokenizer_path.display().to_string();

    let provider = local::LocalEmbeddingProvider::load(local::LocalEmbeddingConfig {
        model_path: model_path.clone().into(),
        tokenizer_path: tokenizer_path.clone().into(),
        def,
        max_sequence_length,
    })
    .map_err(|err| {
        anyhow::anyhow!(
            "{err}\n\nthe local embedding provider needs '{model_path}' and \
             '{tokenizer_path}', or set YORISHIRO_EMBEDDING_PROVIDER=openai to use an \
             OpenAI-compatible endpoint instead"
        )
    })?;
    tracing::info!(provider = "local", %model_path, dimensions, "embedding provider configured");
    Ok(std::sync::Arc::new(provider))
}

/// The model and tokenizer paths the local provider should load, fetching them on first use if that is what this deployment needs.
///
/// Three cases, in order:
///
/// 1. Either path variable is set: use both paths as given and fetch nothing, even if a file is missing.
///    An operator who names a path has told us where the file is, so a typo there must fail loudly.
///    Downloading half a gigabyte to a different location because their path was wrong is worse than refusing to start, and it would leave them debugging a provider that loaded bytes they never pointed at.
///    This asymmetry against the fetching branch below is deliberate, not an omission left to be tidied up later by unifying the two.
/// 2. Nothing is set and the default `models/` path already holds *both* files: use them, fetch nothing.
///    A deployment that placed the files there by hand keeps working exactly as before.
///    Exactly one of them present is an error rather than a fall-through to case 3; see below.
/// 3. Nothing is set and *neither* file is at the default path: fetch into the cache directory under `HOME`, which is both where the download lands and where a later start looks first, so only the first start pays for it.
///
/// `Ok(None)` is case 3 with no resolvable `HOME`, which degrades to [`UnconfiguredEmbeddingProvider`] rather than failing.
/// That is the one degrading path here, and it differs from a failed fetch on whether a retry could ever help: an unresolvable `HOME` is a permanent property of the deployment, so no restart fixes it and the useful answer is a log naming the two variables an operator can set.
/// A network failure or a digest mismatch fails the start instead, so a supervisor's `Restart=on-failure` retries a transient outage and heals by itself, and so that unverified model bytes are never loaded, which is the whole reason the digests are checked.
/// Degrading is right for the unset-provider case [`UnconfiguredEmbeddingProvider`] was built for, but `YORISHIRO_EMBEDDING_PROVIDER=local` is explicit operator intent to run embeddings, so quietly serving a deployment whose search is dead would answer a request nobody made.
/// What the default `models/` path's contents mean for [`resolve_local_paths`].
#[derive(Debug, PartialEq, Eq)]
enum DefaultPathOutcome {
    /// Both files are there: load them, fetch nothing.
    UseBoth,
    /// Exactly one is there, which is an error rather than something to fetch around.
    Incomplete { model_is_present: bool },
    /// Neither is there, so this deployment has not placed anything and the fetch is what it wants.
    Fetch,
}

/// Split out from [`resolve_local_paths`] so the rule can be tested without a filesystem: it is the one decision here where the wrong answer is silent rather than loud.
///
/// Exactly one file present is a half-executed intent, not an empty default path: someone put that file there on purpose and the other is missing.
/// Fetching around it would quietly ignore the file they chose and embed with a different model, which can disagree with the vectors already in the index while every status stays green.
/// A lone file here is a hard error rather than a partial load: an incomplete model directory cannot serve embeddings, so it is reported at boot instead of at first use.
fn default_path_outcome(model_exists: bool, tokenizer_exists: bool) -> DefaultPathOutcome {
    match (model_exists, tokenizer_exists) {
        (true, true) => DefaultPathOutcome::UseBoth,
        (false, false) => DefaultPathOutcome::Fetch,
        (model_is_present, _) => DefaultPathOutcome::Incomplete { model_is_present },
    }
}

/// The default repo-local path a model's files live at when nothing else is configured, scoped by `def.short_id` so two models' default files can never collide or be silently substituted for one another.
/// A deployment that flips `YORISHIRO_LOCAL_MODEL` finds nothing at its new model's default path rather than finding the previous model's files and loading them under the new model's name, which would pass every check this provider runs, since the mismatch is between which weights loaded and what the deployment now believes it configured, not anything visible from the files themselves.
fn default_paths(def: &model_fetch::LocalModelDef) -> (std::path::PathBuf, std::path::PathBuf) {
    let dir = std::path::PathBuf::from("models").join(def.short_id);
    (dir.join("model.safetensors"), dir.join("tokenizer.json"))
}

async fn resolve_local_paths(
    def: &model_fetch::LocalModelDef,
) -> anyhow::Result<Option<(std::path::PathBuf, std::path::PathBuf)>> {
    let configured_model = std::env::var("YORISHIRO_LOCAL_MODEL_PATH").ok();
    let configured_tokenizer = std::env::var("YORISHIRO_LOCAL_TOKENIZER_PATH").ok();
    let (default_model, default_tokenizer) = default_paths(def);
    if configured_model.is_some() || configured_tokenizer.is_some() {
        return Ok(Some((
            configured_model.map_or(default_model, Into::into),
            configured_tokenizer.map_or(default_tokenizer, Into::into),
        )));
    }

    match default_path_outcome(default_model.exists(), default_tokenizer.exists()) {
        DefaultPathOutcome::UseBoth => return Ok(Some((default_model, default_tokenizer))),
        DefaultPathOutcome::Incomplete { model_is_present } => {
            let (present, missing) = if model_is_present {
                (&default_model, &default_tokenizer)
            } else {
                (&default_tokenizer, &default_model)
            };
            anyhow::bail!(
                "'{}' is present but '{}' is missing: the local embedding provider needs both. \
                 Add the missing file, or remove '{}' to have both fetched automatically, \
                 or set YORISHIRO_LOCAL_MODEL_PATH and YORISHIRO_LOCAL_TOKENIZER_PATH to files \
                 this deployment already has",
                present.display(),
                missing.display(),
                present.display()
            );
        }
        DefaultPathOutcome::Fetch => {}
    }

    match model_fetch::ensure_model_files(def).await? {
        Some(paths) => Ok(Some(paths)),
        None => {
            tracing::warn!(
                "YORISHIRO_EMBEDDING_PROVIDER=local, but the model files are missing and HOME does not resolve, so there is nowhere to fetch them to: set YORISHIRO_LOCAL_MODEL_PATH and YORISHIRO_LOCAL_TOKENIZER_PATH to files this deployment already has. Continuing with no embedding provider; search and recall will error."
            );
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use serial_test::serial;

    use super::*;

    /// `#[serial]`: mutates process-wide environment variables, which races other tests in this
    /// binary that also read or write `YORISHIRO_ONNX_*`/`YORISHIRO_LOCAL_*` if run concurrently.
    #[test]
    #[serial]
    fn reject_renamed_onnx_vars_fails_when_any_old_variable_is_set() {
        for (old, fate) in &RENAMED_ONNX_VARS {
            unsafe {
                std::env::set_var(old, "x");
            }
            let result = reject_renamed_onnx_vars();
            unsafe {
                std::env::remove_var(old);
            }
            let Err(err) = result else {
                panic!("reject_renamed_onnx_vars should fail when {old} is set");
            };
            let message = err.to_string();
            assert!(message.contains(old), "{message}");
            // The two fates must not share a message: `Renamed` claims a wrong-model risk that
            // does not exist for `Removed`, and a reader who sees that claim on an inert variable
            // learns to discount it on the variable where it is actually true.
            match fate {
                OnnxVarFate::Renamed(new) => {
                    assert!(message.contains(new), "{message}");
                    assert!(message.contains("different model"), "{message}");
                }
                OnnxVarFate::Removed => {
                    assert!(message.contains("no longer has any effect"), "{message}");
                    assert!(!message.contains("different model"), "{message}");
                }
            }
        }
    }

    /// `std::env::var` returns `Err` both when a variable is unset and when it is set to a
    /// non-UTF-8 value, so a naive `.is_err()` check would let a non-UTF-8 stale value slip past
    /// this guard as though the variable were absent, exactly the case this test rules out.
    /// Unix-only: building a non-UTF-8 `OsString` from arbitrary bytes needs
    /// `OsStringExt::from_vec`, which only exists on Unix; Windows OS strings are not able to
    /// hold arbitrary invalid UTF-8 the same way, so this scenario cannot arise there the same way.
    #[test]
    #[serial]
    #[cfg(unix)]
    fn reject_renamed_onnx_vars_catches_a_non_utf8_value() {
        use std::os::unix::ffi::OsStringExt;

        let (old, _fate) = &RENAMED_ONNX_VARS[0];
        let non_utf8 = std::ffi::OsString::from_vec(vec![0xFF, 0xFE, 0xFD]);
        unsafe {
            std::env::set_var(old, &non_utf8);
        }
        let result = reject_renamed_onnx_vars();
        unsafe {
            std::env::remove_var(old);
        }
        let Err(err) = result else {
            panic!("reject_renamed_onnx_vars should fail when {old} is set to a non-UTF-8 value");
        };
        assert!(err.to_string().contains(old));
    }

    #[test]
    #[serial]
    fn reject_renamed_onnx_vars_passes_when_none_are_set() {
        for (old, _fate) in &RENAMED_ONNX_VARS {
            unsafe {
                std::env::remove_var(old);
            }
        }
        assert!(reject_renamed_onnx_vars().is_ok());
    }

    /// The half-populated cases are the reason this rule exists: falling through to the fetch there would ignore a file an operator deliberately placed and embed with a different model, with nothing in any status to show for it.
    #[test]
    fn a_lone_file_at_the_default_path_is_an_error_not_a_fetch() {
        assert_eq!(
            default_path_outcome(true, true),
            DefaultPathOutcome::UseBoth
        );
        assert_eq!(
            default_path_outcome(false, false),
            DefaultPathOutcome::Fetch
        );
        assert_eq!(
            default_path_outcome(true, false),
            DefaultPathOutcome::Incomplete {
                model_is_present: true
            },
            "a lone model must not fall through to the fetch"
        );
        assert_eq!(
            default_path_outcome(false, true),
            DefaultPathOutcome::Incomplete {
                model_is_present: false
            },
            "a lone tokenizer must not fall through to the fetch"
        );
    }

    /// `#[serial]`: mutates `YORISHIRO_LOCAL_MODEL`, which races other tests in this binary that also touch it if run concurrently.
    #[test]
    #[serial]
    fn resolve_local_model_rejects_an_unknown_value() {
        unsafe {
            std::env::set_var("YORISHIRO_LOCAL_MODEL", "not-a-real-model");
        }
        let result = resolve_local_model();
        unsafe {
            std::env::remove_var("YORISHIRO_LOCAL_MODEL");
        }
        let Err(err) = result else {
            panic!("resolve_local_model should fail for an unrecognized YORISHIRO_LOCAL_MODEL");
        };
        let message = err.to_string();
        assert!(message.contains("not-a-real-model"), "{message}");
        for def in model_fetch::MODELS {
            assert!(
                message.contains(def.short_id),
                "error should list {} as a valid value: {message}",
                def.short_id
            );
        }
    }

    /// An unset `YORISHIRO_LOCAL_MODEL` resolves to `model_fetch::DEFAULT_MODEL`.
    /// Asserting the concrete default (nomic, as of this commit) rather than just "resolves to something" makes a future flip to a different default a visible test change here, not a silent one.
    #[test]
    #[serial]
    fn resolve_local_model_defaults_when_unset() {
        unsafe {
            std::env::remove_var("YORISHIRO_LOCAL_MODEL");
        }
        let def = resolve_local_model().expect("default resolution must not fail");
        assert_eq!(def.short_id, model_fetch::DEFAULT_MODEL.short_id);
        assert_eq!(def.short_id, model_fetch::NOMIC.short_id);
    }
}
