//! First-use fetch of the local embedding provider's model and tokenizer.
//!
//! `YORISHIRO_EMBEDDING_PROVIDER=local` needs a safetensors checkpoint and its tokenizer, neither of which is in the repository: the model checkpoint is about 1.1 GB, which does not belong in git.
//! Rather than making an operator fetch them by hand, the default path is fetched on first use and verified against a hardcoded digest.
//!
//! The model and tokenizer files are fetched into `models/<short_id>/` on first use when absent.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use sha2::{Digest, Sha256};

/// One file to fetch, with everything needed to verify it.
#[derive(Clone, Copy)]
pub struct Artifact {
    /// Path within the model repository, at the owning [`LocalModelDef::revision`].
    pub remote_path: &'static str,
    /// Name under the cache directory.
    pub local_name: &'static str,
    /// SHA256 of the file's bytes.
    ///
    /// Taken by downloading the file and running `sha256sum` on it, never transcribed from a response header.
    /// The headers cannot be trusted for this. `model.safetensors` answers with two different 64-hex values (`x-linked-etag` on the 302 and `etag` on the 200), and nothing in the response says which is the content digest; it is the 302's. A small tokenizer file's ETag matches neither, being a 40-hex Git blob SHA1, since it is small enough not to go through LFS.
    /// Updating the owning definition's revision means re-measuring these the same way.
    pub sha256: &'static str,
    /// Expected length in bytes, checked before hashing.
    ///
    /// A truncated download is the ordinary failure at this size, and a length comparison rejects one without reading the whole file back through SHA256 first.
    pub size: u64,
    /// What to call this in a log line an operator reads.
    pub description: &'static str,
}

/// One selectable local model: everything `super::build_local_provider` and [`super::local`] need to fetch, load, and identify it, and nothing that lives outside this module reaches for a bare `REPO`/`REVISION` constant instead.
///
/// Model and tokenizer artifacts share one definition so their pinned revisions stay together.
pub struct LocalModelDef {
    /// The model identifier reported by [`super::EmbeddingProvider::model_name`] and stamped onto a workspace at creation.
    /// Uses the HuggingFace repository identifier independently of the inference engine.
    pub(super) id: &'static str,
    /// Selects this definition via `YORISHIRO_LOCAL_MODEL`, and names its own cache/default-path subdirectory.
    /// Short and filesystem-safe, unlike `Self::id`, which contains a `/`.
    pub short_id: &'static str,
    /// The model revision this deployment pins.
    ///
    /// A tag or `main` would let the bytes behind these digests change under us, turning a legitimate upstream update into a verification failure that looks like corruption.
    pub revision: &'static str,
    pub model: Artifact,
    pub tokenizer: Artifact,
    /// Output vector width stamped on workspaces.
    pub(super) dimensions: usize,
    /// Usable token limit after reserving XLM-RoBERTa special positions.
    pub(super) max_sequence_length: usize,
    /// Prepended to a query text before embedding it (`EmbedKind::Query`), empty for a model with no such convention.
    pub(super) query_prefix: &'static str,
    /// Prepended to a document text before embedding it (`EmbedKind::Document`), empty for a model with no such convention.
    pub(super) document_prefix: &'static str,
}

/// Pinned multilingual-e5-base artifacts and inference conventions.
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
};

/// The local model used by this deployment.
pub const DEFAULT_MODEL: &LocalModelDef = &MULTILINGUAL_E5_BASE;

pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
pub const READ_TIMEOUT: Duration = Duration::from_secs(60);

static NEXT_STAGING_NONCE: AtomicU64 = AtomicU64::new(0);

/// Where an auto-fetched model lives, scoped to `def` so two models' cached files can never collide or be mismatched with each other.
///
pub(crate) fn cache_dir(def: &LocalModelDef) -> PathBuf {
    PathBuf::from("models").join(def.short_id)
}

/// The model and tokenizer paths to load from, fetching either one first if it is absent.
///
/// A failed fetch fails startup.
pub(super) async fn ensure_model_files(def: &LocalModelDef) -> anyhow::Result<(PathBuf, PathBuf)> {
    let dir = cache_dir(def);

    let client = reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(READ_TIMEOUT)
        .build()?;
    let model = ensure_file(&dir, def, &def.model, &client).await?;
    let tokenizer = ensure_file(&dir, def, &def.tokenizer, &client).await?;
    Ok((model, tokenizer))
}

async fn ensure_file(
    dir: &Path,
    def: &LocalModelDef,
    artifact: &Artifact,
    client: &reqwest::Client,
) -> anyhow::Result<PathBuf> {
    let url = format!(
        "https://huggingface.co/{}/resolve/{}/{}",
        def.id, def.revision, artifact.remote_path
    );
    ensure_file_with_client(dir, &url, artifact, client).await
}

///
/// # Errors
/// Returns an error if the operation cannot be completed.
pub async fn ensure_file_with_client(
    dir: &Path,
    url: &str,
    artifact: &Artifact,
    client: &reqwest::Client,
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

    let result = download_verified(url, &temp, artifact, client).await;
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

/// How long a `.partial.` file must have gone untouched before a sweep will remove it.
///
/// Long enough that an in-flight download is never a candidate: the sweep only ever looks at files whose mtime has not moved for this long, and a live download writes continuously.
pub const STALE_PARTIAL_AGE: std::time::Duration = std::time::Duration::from_secs(6 * 60 * 60);

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
pub fn sweep_stale_partials(dir: &Path, artifact: &Artifact) {
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
///
/// # Errors
/// Returns an error if the operation cannot be completed.
async fn download_verified(
    url: &str,
    temp: &Path,
    artifact: &Artifact,
    client: &reqwest::Client,
) -> anyhow::Result<()> {
    use tokio::io::AsyncWriteExt as _;

    let mut response = client.get(url).send().await.map_err(|err| {
        let kind = if err.is_timeout() {
            "request timed out"
        } else {
            "request failed"
        };
        anyhow::anyhow!("failed to fetch {url}: {kind}: {err}")
    })?;
    if !response.status().is_success() {
        // Do not read or include an error body. It can contain upstream credentials or request data.
        anyhow::bail!("failed to fetch {url}: HTTP {}", response.status());
    }

    let mut file = tokio::fs::File::create(temp)
        .await
        .map_err(|err| anyhow::anyhow!("failed to create {}: {err}", temp.display()))?;

    // Streamed chunk by chunk rather than collected with `bytes()`: buffering hundreds of megabytes in memory only to write it straight back out costs the whole model's size in resident memory for no benefit.
    let mut hasher = Sha256::new();
    let mut written: u64 = 0;
    while let Some(chunk) = response.chunk().await.map_err(|err| {
        let kind = if err.is_timeout() {
            "request timed out"
        } else {
            "response stream failed"
        };
        anyhow::anyhow!("failed while downloading {url}: {kind}: {err}")
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

    let digest = crate::data::encoding::hex(hasher.finalize());
    if digest != artifact.sha256 {
        anyhow::bail!(
            "{url} has SHA256 {digest}, expected {}: the download is corrupt or the file has been tampered with",
            artifact.sha256
        );
    }

    Ok(())
}
