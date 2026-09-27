//! First-use fetch of the local embedding provider's model and tokenizer.
//!
//! `YORISHIRO_EMBEDDING_PROVIDER=local` needs a safetensors checkpoint and its tokenizer, neither of which is in the repository: the smaller of the two models here is about 522 MiB, which does not belong in git.
//! Rather than making an operator fetch them by hand, the default path is fetched on first use and verified against a hardcoded digest.
//!
//! The model and tokenizer files live at `models/<short_id>/model.safetensors` and `models/<short_id>/tokenizer.json` for the selected model, or are fetched into `$HOME/.cache/yorishiro/models/<short_id>/` on first use when absent.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use sha2::{Digest, Sha256};

/// One file to fetch, with everything needed to verify it.
#[derive(Clone, Copy)]
pub(super) struct Artifact {
    /// Path within the model repository, at the owning [`LocalModelDef::revision`].
    remote_path: &'static str,
    /// Name under the cache directory.
    local_name: &'static str,
    /// SHA256 of the file's bytes.
    ///
    /// Taken by downloading the file and running `sha256sum` on it, never transcribed from a response header.
    /// The headers cannot be trusted for this. `model.safetensors` answers with two different 64-hex values (`x-linked-etag` on the 302 and `etag` on the 200), and nothing in the response says which is the content digest; it is the 302's. A small tokenizer file's ETag matches neither, being a 40-hex Git blob SHA1, since it is small enough not to go through LFS.
    /// Updating the owning definition's revision means re-measuring these the same way.
    sha256: &'static str,
    /// Expected length in bytes, checked before hashing.
    ///
    /// A truncated download is the ordinary failure at this size, and a length comparison rejects one without reading the whole file back through SHA256 first.
    size: u64,
    /// What to call this in a log line an operator reads.
    description: &'static str,
}

/// One selectable local model: everything [`super::build_local_provider`] and [`super::local`] need to fetch, load, and identify it, and nothing that lives outside this module reaches for a bare `REPO`/`REVISION` constant instead.
///
/// The model and tokenizer artifacts live on the same definition rather than as two independent statics, deliberately: both output 768 dimensions on the two models defined below, so a mismatched model/tokenizer pairing would pass every shape check silently, embedding with the wrong vocabulary while looking healthy.
/// Pairing them on one struct makes that swap a compile-time impossibility rather than a runtime risk to guard against.
pub struct LocalModelDef {
    /// The model identifier reported by [`super::EmbeddingProvider::model_name`] and stamped onto a workspace at creation.
    /// A HuggingFace repo id, since that is the only identifier that survives an implementation change (this codebase's own `ort` to `candle` migration already outlived one such identifier).
    pub(super) id: &'static str,
    /// Selects this definition via `YORISHIRO_LOCAL_MODEL`, and names its own cache/default-path subdirectory.
    /// Short and filesystem-safe, unlike [`Self::id`], which contains a `/`.
    pub(super) short_id: &'static str,
    /// The model revision this deployment pins.
    ///
    /// A tag or `main` would let the bytes behind these digests change under us, turning a legitimate upstream update into a verification failure that looks like corruption.
    revision: &'static str,
    model: Artifact,
    tokenizer: Artifact,
    /// Output vector width. Both definitions below happen to produce 768, which is what lets a deployment mix them in one `entity_entities.embedding vector(768)` column at all; see the write-time model check in `services/embedding/sync.rs` for why that coincidence still needs guarding.
    pub(super) dimensions: usize,
    /// Upper bound on tokenized sequence length before truncation.
    ///
    /// Not always the model's raw position limit: nomic-embed-text-v1.5's `n_positions` (8192) is the literal rotary-embedding table size, safe to use directly.
    /// multilingual-e5-base's `max_position_embeddings` (514) is *not* directly usable this way: XLM-RoBERTa reserves two of those positions (a `bos`/start position and a `pad` position, offset into the position-id scheme by `pad_token_id`), so the model's own `sentence_bert_config.json` publishes the already-adjusted usable length (512) instead.
    /// Each definition below carries whichever figure is actually safe to truncate to, not a uniform "the config.json value"; do not "simplify" this into one shared field name without re-deriving both numbers.
    pub(super) max_sequence_length: usize,
    /// Prepended to a query text before embedding it (`EmbedKind::Query`), empty for a model with no such convention.
    pub(super) query_prefix: &'static str,
    /// Prepended to a document text before embedding it (`EmbedKind::Document`), empty for a model with no such convention.
    pub(super) document_prefix: &'static str,
    /// Which `candle-transformers` architecture loads this checkpoint.
    pub(super) architecture: Architecture,
}

/// The `candle-transformers` model family a [`LocalModelDef`] loads through.
/// A backend branch on this stays internal to `local.rs`'s own load/forward code, per this repository's own rule that a backend distinction must not change the function signature or return type for callers.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Architecture {
    /// `candle_transformers::models::xlm_roberta::XLMRobertaModel`.
    XlmRoberta,
}

/// `intfloat/multilingual-e5-base`, this provider's default; see [`DEFAULT_MODEL`]'s own doc comment for why.
/// Multilingual (250,002-token XLM-RoBERTa vocabulary, entirely different from nomic's), which is the point of making it the default: this codebase's search and recall are not English-only.
pub(super) static MULTILINGUAL_E5_BASE: LocalModelDef = LocalModelDef {
    id: "intfloat/multilingual-e5-base",
    short_id: "multilingual-e5-base",
    revision: "d128750597153bb5987e10b1c3493a34e5a4502a",
    model: Artifact {
        remote_path: "model.safetensors",
        local_name: "model.safetensors",
        sha256: "a18a44fad1d0b46ded15928144138cff1135d5cc8233bdd90be5f18822de09a7",
        size: 1_112_201_288,
        description: "model",
    },
    tokenizer: Artifact {
        remote_path: "tokenizer.json",
        local_name: "tokenizer.json",
        sha256: "62c24cdc13d4c9952d63718d6c9fa4c287974249e16b7ade6d5a85e7bbb75626",
        size: 17_082_660,
        description: "tokenizer",
    },
    dimensions: crate::services::embedding::DEFAULT_EMBEDDING_DIMENSIONS,
    // `sentence_bert_config.json`'s `max_seq_length`, not `config.json`'s `max_position_embeddings` (514): XLM-RoBERTa reserves two position ids (bos/pad, offset from `pad_token_id`), so only 512 of the 514 are actually usable.
    // 514 - 2 = 512, confirmed against both files at this revision; do not switch this back to the raw `max_position_embeddings` value.
    max_sequence_length: 512,
    // intfloat/multilingual-e5-base's documented convention (its own model card): a query and a stored document are embedded asymmetrically, or retrieval quality degrades silently (the vectors are still the right shape and still normalize either way).
    query_prefix: "query: ",
    document_prefix: "passage: ",
    architecture: Architecture::XlmRoberta,
};

/// The default model when `YORISHIRO_LOCAL_MODEL` is unset.
///
/// `multilingual-e5-base`, not `nomic-embed-text-v1.5`: this codebase's search and recall are not English-only, and only the multilingual model serves that well.
/// `multilingual-e5-base` is used instead of `nomic-embed-text-v1.5` because this codebase's search and recall are not English-only.
/// The write-time model check (`services/embedding/sync.rs`) refuses a write whose vector doesn't match the workspace's stamped model, and the `reindex_embeddings` task moves a workspace between models: together these prevent the "stamp says one model, data holds another" failure.
/// A workspace with no stamp (both `embedding_model` and `embedding_dimensions` are `NULL`) inherits the deployment default, so it is also protected: writes go through the stamp that `sync_embedding` sets on the first embed, and the model check compares against that stamp.
/// `docs/configuration.md`'s "Moving a workspace between embedding models" section documents the procedure for every other workspace: change configuration, restart, then reindex.
pub(super) const DEFAULT_MODEL: &LocalModelDef = &MULTILINGUAL_E5_BASE;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

static NEXT_STAGING_NONCE: AtomicU64 = AtomicU64::new(0);

/// Where an auto-fetched model lives, scoped to `def` so two models' cached files can never collide or be mismatched with each other.
///
/// Returns `None` when `HOME` does not resolve.
/// The Docker image sets `HOME` for its service user for exactly this reason, but nothing here may assume that holds for every deployment: an operator running the binary directly, under a different supervisor, or in a container image of their own can still reach a user with no home.
/// Nothing invents a fallback directory in that case; the caller degrades and names the two path variables instead, since writing hundreds of megabytes to a guessed location is worse than not writing it.
pub(crate) fn cache_dir(def: &LocalModelDef) -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    if home.is_empty() {
        return None;
    }
    Some(
        PathBuf::from(home)
            .join(".cache/yorishiro/models")
            .join(def.short_id),
    )
}

/// The model and tokenizer paths to load from, fetching either one first if it is absent.
///
/// `Ok(None)` means the destination could not be resolved at all (no `HOME`), so the caller degrades rather than failing.
/// `Err` means a fetch was attempted and failed, which fails the boot; see [`super::build_local_provider`] for why those two outcomes differ.
pub(super) async fn ensure_model_files(
    def: &LocalModelDef,
) -> anyhow::Result<Option<(PathBuf, PathBuf)>> {
    let Some(dir) = cache_dir(def) else {
        return Ok(None);
    };

    let model = ensure_file(&dir, def, &def.model).await?;
    let tokenizer = ensure_file(&dir, def, &def.tokenizer).await?;
    Ok(Some((model, tokenizer)))
}

/// Fetches one artifact into `dir` unless it is already there, and returns its path.
async fn ensure_file(
    dir: &Path,
    def: &LocalModelDef,
    artifact: &Artifact,
) -> anyhow::Result<PathBuf> {
    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .expect("reqwest client configuration is static and always valid");
    ensure_file_with_http(dir, def, artifact, &client, &ReqwestArtifactHttp).await
}

async fn ensure_file_with_http(
    dir: &Path,
    def: &LocalModelDef,
    artifact: &Artifact,
    client: &reqwest::Client,
    http: &dyn ArtifactHttp,
) -> anyhow::Result<PathBuf> {
    let destination = dir.join(artifact.local_name);
    if destination.exists() {
        // The size is checked, and deliberately not the digest. Do not "fix" this into a re-verification: the reasons it is not one are the whole point.
        //
        // The digest guards the wire, not the disk. This destination only ever receives bytes that already passed both length and SHA256, moved in by an atomic rename within one filesystem, so the mechanism cannot itself produce a bad file here.
        // Getting one needs an outside writer or disk corruption, and corruption that mangles a safetensors header fails loudly in `LocalEmbeddingProvider::load` regardless.
        // The quiet case, a valid but different model swapped in, needs someone holding the service user's write access, and they could as easily replace the `models/<short_id>/` files, which is not checked at all.
        //
        // Re-hashing only this tier would also invert the design: files an operator placed themselves are deliberately unverified, since a custom model cannot match a digest pinned to this definition's, so re-checking at read time the one tier that was already verified at write time, at hundreds of megabytes on every single start forever, would spend the cost exactly where it buys least.
        //
        // The size check earns its place at a different price: `stat` is free, and it catches truncation, which is what an interrupted write actually leaves behind.
        // Anything past that is not worth paying for on every start.
        match std::fs::metadata(&destination) {
            Ok(meta) if meta.len() == artifact.size => return Ok(destination),
            Ok(meta) => {
                // Removing it rather than failing: a bad cache file is the case a retry can actually fix, so refetching heals it in place instead of needing an operator to find and delete the file first.
                tracing::warn!(
                    path = %destination.display(),
                    found = meta.len(),
                    expected = artifact.size,
                    "cached {} is the wrong size; removing it and fetching again",
                    artifact.description
                );
                std::fs::remove_file(&destination).map_err(|err| {
                    anyhow::anyhow!(
                        "failed to remove the corrupt cached file {}: {err}",
                        destination.display()
                    )
                })?;
            }
            Err(err) => {
                anyhow::bail!("failed to read {}: {err}", destination.display());
            }
        }
    }

    std::fs::create_dir_all(dir)
        .map_err(|err| anyhow::anyhow!("failed to create {}: {err}", dir.display()))?;

    let url = format!(
        "https://huggingface.co/{}/resolve/{}/{}",
        def.id, def.revision, artifact.remote_path
    );
    let mebibytes = artifact.size / (1024 * 1024);
    tracing::info!(
        url = %url,
        destination = %destination.display(),
        "fetching the local embedding provider's {} ({mebibytes} MiB); startup blocks until this finishes",
        artifact.description
    );

    sweep_stale_partials(dir, artifact);

    // The temp file sits in the destination's own directory rather than the system temp directory, because `rename` is only atomic within a filesystem.
    // The pid and process-local nonce keep concurrent fetches from writing or cleaning up the same partial file.
    let temp = staging_path(dir, artifact);

    let result = download_verified(&url, &temp, artifact, client, http).await;
    if result.is_err() {
        // A partial or corrupt file must not survive to be mistaken for a complete one on the next startup.
        let _ = std::fs::remove_file(&temp);
    }
    result?;

    std::fs::rename(&temp, &destination).map_err(|err| {
        let _ = std::fs::remove_file(&temp);
        anyhow::anyhow!("failed to move {} into place: {err}", destination.display())
    })?;

    tracing::info!(destination = %destination.display(), "fetched the {}", artifact.description);
    Ok(destination)
}

fn staging_path(dir: &Path, artifact: &Artifact) -> PathBuf {
    let nonce = NEXT_STAGING_NONCE.fetch_add(1, Ordering::Relaxed);
    dir.join(format!(
        "{}.partial.{}.{}",
        artifact.local_name,
        std::process::id(),
        nonce
    ))
}

#[async_trait]
trait ArtifactHttp: Send + Sync {
    async fn get(
        &self,
        client: &reqwest::Client,
        url: &str,
    ) -> Result<ArtifactHttpResponse, ArtifactHttpError>;
}

struct ReqwestArtifactHttp;

#[async_trait]
impl ArtifactHttp for ReqwestArtifactHttp {
    async fn get(
        &self,
        client: &reqwest::Client,
        url: &str,
    ) -> Result<ArtifactHttpResponse, ArtifactHttpError> {
        let response = client
            .get(url)
            .send()
            .await
            .map_err(ArtifactHttpError::from_request)?;
        let status = response.status();
        Ok(ArtifactHttpResponse {
            status,
            body: Box::new(ReqwestArtifactBody(response)),
        })
    }
}

struct ArtifactHttpResponse {
    status: reqwest::StatusCode,
    body: Box<dyn ArtifactBody>,
}

#[async_trait]
trait ArtifactBody: Send {
    async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>, ArtifactHttpError>;
}

struct ReqwestArtifactBody(reqwest::Response);

#[async_trait]
impl ArtifactBody for ReqwestArtifactBody {
    async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>, ArtifactHttpError> {
        self.0
            .chunk()
            .await
            .map(|chunk| chunk.map(|chunk| chunk.to_vec()))
            .map_err(ArtifactHttpError::from_response)
    }
}

#[derive(Debug)]
enum ArtifactHttpError {
    Timeout(String),
    Request(String),
    Response(String),
}

impl ArtifactHttpError {
    fn from_request(error: reqwest::Error) -> Self {
        if error.is_timeout() {
            Self::Timeout(error.to_string())
        } else {
            Self::Request(error.to_string())
        }
    }

    fn from_response(error: reqwest::Error) -> Self {
        if error.is_timeout() {
            Self::Timeout(error.to_string())
        } else {
            Self::Response(error.to_string())
        }
    }

    fn message(&self) -> &str {
        match self {
            Self::Timeout(message) | Self::Request(message) | Self::Response(message) => message,
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            Self::Timeout(_) => "request timed out",
            Self::Request(_) => "request failed",
            Self::Response(_) => "response stream failed",
        }
    }
}

/// How long a `.partial.` file must have gone untouched before a sweep will remove it.
///
/// Long enough that an in-flight download is never a candidate: the sweep only ever looks at files whose mtime has not moved for this long, and a live download writes continuously.
const STALE_PARTIAL_AGE: std::time::Duration = std::time::Duration::from_secs(6 * 60 * 60);

/// Removes abandoned temp files left by earlier killed downloads.
///
/// A killed download leaves `<name>.partial.<pid>.<nonce>`, and a subsequent start has a different staging name, so without this nothing ever reuses or removes it: on a deployment whose network keeps dropping mid-fetch, that is hundreds of megabytes of dead bytes per attempt, accumulating forever.
///
/// The age check is what keeps this away from a download that is still running.
/// Two processes starting together (a server and a worker, or `--server-and-worker` alongside a task) each write their own pid-and-nonce-suffixed file, and a sweep that removed a live one would fail the other's rename with `ENOENT`.
/// Requiring [`STALE_PARTIAL_AGE`] of no writes means an in-flight download is never a candidate, since it is writing continuously; anything that old belongs to a process that is gone.
/// Even in the case where that is somehow wrong, the consequence is bounded: the rename fails, that start fails with it, and on a subsequent attempt a supervisor restart finds the destination already there or fetches it cleanly.
///
/// Every failure here is ignored: a directory that cannot be read, or a file that cannot be removed, must not stop a fetch that is otherwise fine.
fn sweep_stale_partials(dir: &Path, artifact: &Artifact) {
    let prefix = format!("{}.partial.", artifact.local_name);
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if !entry.file_name().to_string_lossy().starts_with(&prefix) {
            continue;
        }
        let stale = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .and_then(|modified| modified.elapsed().map_err(std::io::Error::other))
            .is_ok_and(|age| age >= STALE_PARTIAL_AGE);
        if stale {
            tracing::info!(path = %entry.path().display(), "removing an abandoned partial download");
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Streams `url` into `temp` and checks the result against `artifact`'s expected length and digest.
async fn download_verified(
    url: &str,
    temp: &Path,
    artifact: &Artifact,
    client: &reqwest::Client,
    http: &dyn ArtifactHttp,
) -> anyhow::Result<()> {
    use tokio::io::AsyncWriteExt as _;

    let mut response = http.get(client, url).await.map_err(|err| {
        anyhow::anyhow!("failed to fetch {url}: {}: {}", err.kind(), err.message())
    })?;
    if !response.status.is_success() {
        // Do not read or include an error body. It can contain upstream credentials or request data.
        anyhow::bail!("failed to fetch {url}: HTTP {}", response.status);
    }

    let mut file = tokio::fs::File::create(temp)
        .await
        .map_err(|err| anyhow::anyhow!("failed to create {}: {err}", temp.display()))?;

    // Streamed chunk by chunk rather than collected with `bytes()`: buffering hundreds of megabytes in memory only to write it straight back out costs the whole model's size in resident memory for no benefit.
    let mut hasher = Sha256::new();
    let mut written: u64 = 0;
    while let Some(chunk) = response.body.next_chunk().await.map_err(|err| {
        anyhow::anyhow!(
            "failed while downloading {url}: {}: {}",
            err.kind(),
            err.message()
        )
    })? {
        hasher.update(&chunk);
        written += chunk.len() as u64;
        file.write_all(&chunk)
            .await
            .map_err(|err| anyhow::anyhow!("failed writing {}: {err}", temp.display()))?;
    }
    file.flush()
        .await
        .map_err(|err| anyhow::anyhow!("failed writing {}: {err}", temp.display()))?;

    // Length first: it rejects the common truncated-download case without hashing the whole file, and its message says something more useful than a digest mismatch would.
    if written != artifact.size {
        anyhow::bail!(
            "{url} downloaded {written} bytes, expected {}: the download did not complete",
            artifact.size
        );
    }

    let digest = hex_encode(&hasher.finalize());
    if digest != artifact.sha256 {
        anyhow::bail!(
            "{url} has SHA256 {digest}, expected {}: the download is corrupt or the file has been tampered with",
            artifact.sha256
        );
    }

    Ok(())
}

fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::fs;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::SystemTime;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::sync::Barrier;

    use super::*;
    use tempfile::tempdir;

    struct FakeHttp {
        response: std::sync::Mutex<Option<Result<ArtifactHttpResponse, ArtifactHttpError>>>,
        urls: std::sync::Mutex<Vec<String>>,
    }

    #[async_trait]
    impl ArtifactHttp for FakeHttp {
        async fn get(
            &self,
            _client: &reqwest::Client,
            url: &str,
        ) -> Result<ArtifactHttpResponse, ArtifactHttpError> {
            self.urls.lock().unwrap().push(url.to_owned());
            self.response.lock().unwrap().take().unwrap()
        }
    }

    struct FakeBody {
        chunks: VecDeque<Result<Vec<u8>, ArtifactHttpError>>,
        on_next: Option<Arc<dyn Fn() + Send + Sync>>,
    }

    #[async_trait]
    impl ArtifactBody for FakeBody {
        async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>, ArtifactHttpError> {
            if let Some(on_next) = &self.on_next {
                on_next();
            }
            self.chunks.pop_front().transpose()
        }
    }

    fn client() -> reqwest::Client {
        reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .unwrap()
    }

    fn artifact(bytes: &[u8]) -> Artifact {
        Artifact {
            remote_path: "test.bin",
            local_name: "test.bin",
            sha256: Box::leak(hex_encode(&Sha256::digest(bytes)).into_boxed_str()),
            size: bytes.len() as u64,
            description: "test artifact",
        }
    }

    fn response(
        status: u16,
        chunks: impl IntoIterator<Item = Result<Vec<u8>, ArtifactHttpError>>,
    ) -> ArtifactHttpResponse {
        ArtifactHttpResponse {
            status: reqwest::StatusCode::from_u16(status).unwrap(),
            body: Box::new(FakeBody {
                chunks: chunks.into_iter().collect(),
                on_next: None,
            }),
        }
    }

    fn fake(response: Result<ArtifactHttpResponse, ArtifactHttpError>) -> FakeHttp {
        FakeHttp {
            response: std::sync::Mutex::new(Some(response)),
            urls: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn assert_no_partials(dir: &Path) {
        assert!(!fs::read_dir(dir).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".partial.")
        }));
    }

    #[test]
    fn hex_encode_pads_single_digit_bytes() {
        assert_eq!(hex_encode(&[0x00, 0x0f, 0xff, 0xa5]), "000fffa5");
        assert_eq!(hex_encode(&[]), "");
    }

    #[test]
    fn configured_artifacts_use_lowercase_sha256_hex() {
        for artifact in [&DEFAULT_MODEL.model, &DEFAULT_MODEL.tokenizer] {
            assert_eq!(artifact.sha256.len(), 64, "{}", artifact.description);
            assert!(
                artifact
                    .sha256
                    .chars()
                    .all(|c| c.is_ascii_digit() || (c.is_ascii_lowercase() && c <= 'f')),
                "{} digest must be lowercase hex",
                artifact.description
            );
        }
        assert_ne!(DEFAULT_MODEL.model.sha256, DEFAULT_MODEL.tokenizer.sha256);
    }

    #[test]
    fn sweep_removes_only_stale_partials_for_one_artifact() {
        let dir = tempdir().unwrap();
        let stale = dir.path().join("test.bin.partial.old");
        let fresh = dir.path().join("test.bin.partial.live");
        let unrelated = dir.path().join("other.bin.partial.old");
        fs::write(&stale, b"old").unwrap();
        fs::write(&fresh, b"live").unwrap();
        fs::write(&unrelated, b"other").unwrap();
        fs::File::open(&stale)
            .unwrap()
            .set_modified(SystemTime::now() - STALE_PARTIAL_AGE - Duration::from_secs(1))
            .unwrap();

        sweep_stale_partials(dir.path(), &artifact(b"payload"));

        assert!(!stale.exists());
        assert!(fresh.exists());
        assert!(unrelated.exists());
    }

    #[tokio::test]
    async fn transport_failure_maps_timeout_and_cleans_staging() {
        let dir = tempdir().unwrap();
        let artifact = artifact(b"payload");
        let http = fake(Err(ArtifactHttpError::Timeout(
            "operation timed out".into(),
        )));

        let error = ensure_file_with_http(dir.path(), DEFAULT_MODEL, &artifact, &client(), &http)
            .await
            .unwrap_err()
            .to_string();

        assert!(error.contains("request timed out"), "{error}");
        assert_no_partials(dir.path());
    }

    #[tokio::test]
    async fn response_stream_timeout_keeps_timeout_mapping() {
        let dir = tempdir().unwrap();
        let artifact = artifact(b"payload");
        let http = fake(Ok(response(
            200,
            [Err(ArtifactHttpError::Timeout("body timed out".into()))],
        )));

        let error = ensure_file_with_http(dir.path(), DEFAULT_MODEL, &artifact, &client(), &http)
            .await
            .unwrap_err()
            .to_string();

        assert!(error.contains("request timed out"), "{error}");
        assert_no_partials(dir.path());
    }

    #[tokio::test]
    async fn non_success_status_does_not_expose_or_consume_response_body() {
        let dir = tempdir().unwrap();
        let consumed = Arc::new(AtomicBool::new(false));
        let body_consumed = Arc::clone(&consumed);
        let response = ArtifactHttpResponse {
            status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            body: Box::new(FakeBody {
                chunks: vec![Ok(b"secret upstream body".to_vec())]
                    .into_iter()
                    .collect(),
                on_next: Some(Arc::new(move || {
                    body_consumed.store(true, Ordering::SeqCst);
                })),
            }),
        };
        let http = fake(Ok(response));
        let error = ensure_file_with_http(
            dir.path(),
            DEFAULT_MODEL,
            &artifact(b"payload"),
            &client(),
            &http,
        )
        .await
        .unwrap_err()
        .to_string();

        assert!(error.contains("HTTP 500"), "{error}");
        assert!(!error.contains("secret upstream body"), "{error}");
        assert!(!consumed.load(Ordering::SeqCst));
        assert_no_partials(dir.path());
    }

    #[tokio::test]
    async fn truncated_download_is_rejected_and_removed() {
        let dir = tempdir().unwrap();
        let artifact = artifact(b"complete payload");
        let http = fake(Ok(response(200, [Ok(b"truncated".to_vec())])));

        let error = ensure_file_with_http(dir.path(), DEFAULT_MODEL, &artifact, &client(), &http)
            .await
            .unwrap_err()
            .to_string();

        assert!(error.contains("downloaded 9 bytes, expected 16"), "{error}");
        assert!(!dir.path().join("test.bin").exists());
        assert_no_partials(dir.path());
    }

    #[tokio::test]
    async fn digest_mismatch_is_rejected_and_removed() {
        let dir = tempdir().unwrap();
        let artifact = Artifact {
            sha256: "0000000000000000000000000000000000000000000000000000000000000000",
            ..artifact(b"payload")
        };
        let http = fake(Ok(response(200, [Ok(b"payload".to_vec())])));

        let error = ensure_file_with_http(dir.path(), DEFAULT_MODEL, &artifact, &client(), &http)
            .await
            .unwrap_err()
            .to_string();

        assert!(error.contains("SHA256"), "{error}");
        assert!(!dir.path().join("test.bin").exists());
        assert_no_partials(dir.path());
    }

    #[tokio::test]
    async fn stale_partials_are_removed_and_failed_partials_do_not_survive() {
        let dir = tempdir().unwrap();
        let stale = dir.path().join("test.bin.partial.old");
        fs::write(&stale, b"old partial").unwrap();
        fs::File::open(&stale)
            .unwrap()
            .set_modified(SystemTime::now() - STALE_PARTIAL_AGE - Duration::from_secs(1))
            .unwrap();

        let artifact = artifact(b"payload");
        let http = fake(Ok(response(200, [Ok(b"payload".to_vec())])));
        let path = ensure_file_with_http(dir.path(), DEFAULT_MODEL, &artifact, &client(), &http)
            .await
            .unwrap();
        assert!(path.exists());
        assert!(!stale.exists());
        assert_no_partials(dir.path());

        fs::remove_file(path).unwrap();
        let failing = fake(Ok(response(
            200,
            [Err(ArtifactHttpError::Response("connection lost".into()))],
        )));
        let _ =
            ensure_file_with_http(dir.path(), DEFAULT_MODEL, &artifact, &client(), &failing).await;
        assert_no_partials(dir.path());
    }

    #[tokio::test]
    async fn cache_hit_skips_transport_and_wrong_length_cache_is_replaced() {
        let dir = tempdir().unwrap();
        let bytes = b"payload";
        let artifact = artifact(bytes);
        let destination = dir.path().join(artifact.local_name);
        fs::write(&destination, bytes).unwrap();
        let cached = fake(Err(ArtifactHttpError::Request("must not be called".into())));
        assert_eq!(
            ensure_file_with_http(dir.path(), DEFAULT_MODEL, &artifact, &client(), &cached,)
                .await
                .unwrap(),
            destination
        );
        assert!(cached.urls.lock().unwrap().is_empty());

        fs::write(&destination, b"bad").unwrap();
        let replacement = fake(Ok(response(200, [Ok(bytes.to_vec())])));
        ensure_file_with_http(
            dir.path(),
            DEFAULT_MODEL,
            &artifact,
            &client(),
            &replacement,
        )
        .await
        .unwrap();
        assert_eq!(fs::read(&destination).unwrap(), bytes);
    }

    #[tokio::test]
    async fn verified_bytes_rename_atomically_and_use_pinned_url() {
        let dir = tempdir().unwrap();
        let artifact = artifact(b"payload");
        let destination = dir.path().join(artifact.local_name);
        let checked_before_rename = Arc::new(AtomicBool::new(false));
        let checked = Arc::clone(&checked_before_rename);
        let destination_during_download = destination.clone();
        let partial_dir = dir.path().to_path_buf();
        let response = ArtifactHttpResponse {
            status: reqwest::StatusCode::OK,
            body: Box::new(FakeBody {
                chunks: vec![Ok(b"payload".to_vec())].into_iter().collect(),
                on_next: Some(Arc::new(move || {
                    assert!(!destination_during_download.exists());
                    assert_eq!(partial_files(&partial_dir).len(), 1);
                    checked.store(true, Ordering::SeqCst);
                })),
            }),
        };
        let http = fake(Ok(response));
        ensure_file_with_http(dir.path(), DEFAULT_MODEL, &artifact, &client(), &http)
            .await
            .unwrap();

        assert!(checked_before_rename.load(Ordering::SeqCst));
        assert_eq!(fs::read(destination).unwrap(), b"payload");
        assert_eq!(
            http.urls.lock().unwrap().as_slice(),
            &[
                "https://huggingface.co/intfloat/multilingual-e5-base/resolve/d128750597153bb5987e10b1c3493a34e5a4502a/test.bin"
            ]
        );
    }

    #[tokio::test]
    async fn concurrent_fetches_use_distinct_staging_paths_and_leave_valid_bytes() {
        let dir = tempdir().unwrap();
        let bytes = b"payload";
        let artifact = artifact(bytes);
        let entered = Arc::new(Barrier::new(2));
        let inspected = Arc::new(Barrier::new(2));
        let http = Arc::new(ConcurrentHttp {
            dir: dir.path().to_path_buf(),
            entered: Arc::clone(&entered),
            inspected: Arc::clone(&inspected),
            bytes: bytes.to_vec(),
        });
        let client = client();

        let first =
            ensure_file_with_http(dir.path(), DEFAULT_MODEL, &artifact, &client, http.as_ref());
        let second =
            ensure_file_with_http(dir.path(), DEFAULT_MODEL, &artifact, &client, http.as_ref());
        let (first, second) = tokio::join!(first, second);

        let successes = [first.as_ref(), second.as_ref()]
            .into_iter()
            .filter(|result| result.is_ok())
            .count();
        if cfg!(windows) {
            assert_eq!(
                successes, 1,
                "Windows cannot rename over an existing destination"
            );
        } else {
            assert_eq!(
                successes, 2,
                "same-filesystem rename should be atomic for both fetches"
            );
        }
        assert_eq!(
            fs::read(dir.path().join(artifact.local_name)).unwrap(),
            bytes
        );
        assert_no_partials(dir.path());
    }

    struct ConcurrentHttp {
        dir: PathBuf,
        entered: Arc<Barrier>,
        inspected: Arc<Barrier>,
        bytes: Vec<u8>,
    }

    #[async_trait]
    impl ArtifactHttp for ConcurrentHttp {
        async fn get(
            &self,
            _client: &reqwest::Client,
            _url: &str,
        ) -> Result<ArtifactHttpResponse, ArtifactHttpError> {
            Ok(ArtifactHttpResponse {
                status: reqwest::StatusCode::OK,
                body: Box::new(ConcurrentBody {
                    dir: self.dir.clone(),
                    entered: Arc::clone(&self.entered),
                    inspected: Arc::clone(&self.inspected),
                    bytes: Some(self.bytes.clone()),
                }),
            })
        }
    }

    struct ConcurrentBody {
        dir: PathBuf,
        entered: Arc<Barrier>,
        inspected: Arc<Barrier>,
        bytes: Option<Vec<u8>>,
    }

    #[async_trait]
    impl ArtifactBody for ConcurrentBody {
        async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>, ArtifactHttpError> {
            self.entered.wait().await;
            assert_eq!(partial_files(&self.dir).len(), 2);
            self.inspected.wait().await;
            Ok(self.bytes.take())
        }
    }

    fn partial_files(dir: &Path) -> Vec<PathBuf> {
        fs::read_dir(dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .contains(".partial.")
            })
            .collect()
    }

    async fn loopback_server(response: Vec<u8>) -> (String, tokio::task::JoinHandle<Vec<u8>>) {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 1024];
            loop {
                let read = socket.read(&mut buffer).await.unwrap();
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            socket.write_all(&response).await.unwrap();
            request
        });
        (format!("http://{address}"), task)
    }

    #[tokio::test]
    async fn reqwest_transport_sends_get_to_the_pinned_path_and_streams_body() {
        let response =
            b"HTTP/1.1 200 OK\r\nContent-Length: 13\r\nConnection: close\r\n\r\nhello, world!";
        let (base, request) = loopback_server(response.to_vec()).await;
        let url = format!(
            "{base}/intfloat/multilingual-e5-base/resolve/d128750597153bb5987e10b1c3493a34e5a4502a/model.safetensors"
        );
        let response = ReqwestArtifactHttp.get(&client(), &url).await.unwrap();
        assert!(response.status.is_success());
        let mut body = response.body;
        let mut bytes = Vec::new();
        while let Some(chunk) = body.next_chunk().await.unwrap() {
            bytes.extend_from_slice(&chunk);
        }
        assert_eq!(bytes, b"hello, world!");

        let request = String::from_utf8(request.await.unwrap()).unwrap();
        assert!(request.starts_with("GET /intfloat/multilingual-e5-base/resolve/d128750597153bb5987e10b1c3493a34e5a4502a/model.safetensors HTTP/1.1\r\n"));
    }

    #[tokio::test]
    async fn reqwest_transport_status_does_not_expose_response_body() {
        let response = b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 21\r\nConnection: close\r\n\r\nsecret upstream body";
        let (base, request) = loopback_server(response.to_vec()).await;
        let url = format!("{base}/artifact");
        let temp = tempdir().unwrap();
        let artifact = artifact(b"payload");
        let error = download_verified(
            &url,
            &temp.path().join("artifact.partial"),
            &artifact,
            &client(),
            &ReqwestArtifactHttp,
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(error.contains("HTTP 503"), "{error}");
        assert!(!error.contains("secret upstream body"), "{error}");
        request.await.unwrap();
    }

    #[tokio::test]
    async fn reqwest_transport_maps_request_and_stream_errors() {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let request_error = ReqwestArtifactHttp
            .get(&client(), &format!("http://{address}/artifact"))
            .await
            .err()
            .expect("a closed loopback listener must reject the request");
        assert!(matches!(request_error, ArtifactHttpError::Request(_)));

        let response = b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\nshort";
        let (base, request) = loopback_server(response.to_vec()).await;
        let mut response = ReqwestArtifactHttp
            .get(&client(), &format!("{base}/artifact"))
            .await
            .unwrap();
        let stream_error = loop {
            match response.body.next_chunk().await {
                Ok(Some(_)) => continue,
                Ok(None) => panic!("the truncated response must fail while streaming"),
                Err(error) => break error,
            }
        };
        assert!(matches!(stream_error, ArtifactHttpError::Response(_)));
        request.await.unwrap();
    }

    #[tokio::test]
    async fn reqwest_transport_maps_request_timeout() {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (_socket, _) = listener.accept().await.unwrap();
            tokio::time::sleep(Duration::from_millis(100)).await;
        });
        let short_client = reqwest::Client::builder()
            .timeout(Duration::from_millis(20))
            .build()
            .unwrap();
        let error = ReqwestArtifactHttp
            .get(&short_client, &format!("http://{address}/artifact"))
            .await
            .err()
            .expect("the delayed response must time out");
        assert!(matches!(error, ArtifactHttpError::Timeout(_)));
    }

    #[tokio::test]
    async fn reqwest_transport_maps_stream_timeout() {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\n")
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(100)).await;
        });
        let short_client = reqwest::Client::builder()
            .timeout(Duration::from_millis(20))
            .build()
            .unwrap();
        let mut response = ReqwestArtifactHttp
            .get(&short_client, &format!("http://{address}/artifact"))
            .await
            .unwrap();
        let error = response.body.next_chunk().await.unwrap_err();
        assert!(matches!(error, ArtifactHttpError::Timeout(_)));
    }

    #[test]
    fn artifact_definitions_pin_full_revisions_and_safe_urls() {
        assert_eq!(DEFAULT_MODEL.revision.len(), 40);
        assert!(
            DEFAULT_MODEL
                .revision
                .chars()
                .all(|c| c.is_ascii_hexdigit())
        );
        assert!(
            DEFAULT_MODEL
                .short_id
                .chars()
                .all(|c| { c.is_ascii_alphanumeric() || c == '-' || c == '.' })
        );
    }
}
