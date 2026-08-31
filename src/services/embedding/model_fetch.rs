//! First-use fetch of the local embedding provider's model and tokenizer.
//!
//! `YORISHIRO_EMBEDDING_PROVIDER=local` needs a safetensors checkpoint and its tokenizer, neither of which is in the repository: the smaller of the two models here is about 522 MiB, which does not belong in git.
//! Rather than making an operator fetch them by hand, the default path is fetched on first use and verified against a hardcoded digest.
//!
//! The model and tokenizer files live at `models/<short_id>/model.safetensors` and `models/<short_id>/tokenizer.json` for the selected model, or are fetched into `$HOME/.cache/yorishiro/models/<short_id>/` on first use when absent.

use std::path::{Path, PathBuf};

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
    /// Output vector width. Both definitions below happen to produce 768, which is what lets a deployment mix them in one `content_entities.embedding vector(768)` column at all; see the write-time model check in `services/embedding/sync.rs` for why that coincidence still needs guarding.
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
/// A backend branch on this stays internal to `local.rs`'s own load/forward code, per this repository's own rule that a backend distinction must not leak into callers; every definition below still looks like a plain model description from the outside.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Architecture {
    /// `candle_transformers::models::nomic_bert::NomicBertModel`: a BERT variant with rotary position embeddings and a SwiGLU MLP.
    NomicBert,
    /// `candle_transformers::models::xlm_roberta::XLMRobertaModel`.
    XlmRoberta,
}

/// `nomic-embed-text-v1.5`, this provider's original model, kept selectable rather than removed: an existing nomic-embedded deployment needs a path forward (`YORISHIRO_LOCAL_MODEL=nomic-embed-text-v1.5`, or `reindex_embeddings` to move off it) besides a forced reindex the moment it upgrades, and the write-time model check needs this definition to exist as a comparison target regardless of which model is the default.
/// Prefix-free: nomic-embed-text-v1.5's recommended `search_query:`/`search_document:` prefixes are a known, deliberately deferred gap tracked separately (see this repository's own issue tracker), not implemented here to avoid invalidating the irreplaceable `ort`-parity fixture and every existing nomic-embedded deployment's vectors in the same change that adds multi-model selection.
pub(super) static NOMIC: LocalModelDef = LocalModelDef {
    id: "nomic-ai/nomic-embed-text-v1.5",
    short_id: "nomic-embed-text-v1.5",
    revision: "e9b6763023c676ca8431644204f50c2b100d9aab",
    model: Artifact {
        remote_path: "model.safetensors",
        local_name: "model.safetensors",
        sha256: "9e7d262b1fe5ea350782829496efa831901b77486bbde1cea54a4c822d010d5c",
        size: 546_938_168,
        description: "model",
    },
    tokenizer: Artifact {
        remote_path: "tokenizer.json",
        local_name: "tokenizer.json",
        sha256: "d241a60d5e8f04cc1b2b3e9ef7a4921b27bf526d9f6050ab90f9267a1f9e5c66",
        size: 711_396,
        description: "tokenizer",
    },
    dimensions: 768,
    // nomic-embed-text-v1.5's own `n_positions` (`candle_transformers::models::nomic_bert::Config::default().n_positions`): the rotary embedding table's actual size, safe to truncate to directly.
    max_sequence_length: 8192,
    query_prefix: "",
    document_prefix: "",
    architecture: Architecture::NomicBert,
};

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
    dimensions: 768,
    // `sentence_bert_config.json`'s `max_seq_length`, not `config.json`'s `max_position_embeddings` (514): XLM-RoBERTa reserves two position ids (bos/pad, offset from `pad_token_id`), so only 512 of the 514 are actually usable.
    // 514 - 2 = 512, confirmed against both files at this revision; do not switch this back to the raw `max_position_embeddings` value.
    max_sequence_length: 512,
    // intfloat/multilingual-e5-base's documented convention (its own model card): a query and a stored document are embedded asymmetrically, or retrieval quality degrades silently (the vectors are still the right shape and still normalize either way).
    query_prefix: "query: ",
    document_prefix: "passage: ",
    architecture: Architecture::XlmRoberta,
};

/// Every model this provider can be configured to load, in the order `YORISHIRO_LOCAL_MODEL`'s error message lists them.
pub(super) const MODELS: &[&LocalModelDef] = &[&NOMIC, &MULTILINGUAL_E5_BASE];

/// The default model when `YORISHIRO_LOCAL_MODEL` is unset.
///
/// `multilingual-e5-base`, not `nomic-embed-text-v1.5`: this codebase's search and recall are not English-only, and only the multilingual model serves that well.
/// Flipping this default was withheld until three things existed, in order: a numeric reference fixture and passing parity test for e5 (`tests/fixtures/e5_reference_embeddings.json`), the write-time model check that refuses a write whose vector doesn't match the workspace's stamped model (`services/embedding/sync.rs`), and the `reindex_embeddings` task (with its own tests) that actually moves a workspace between models.
/// All three now exist, which is what makes this flip safe rather than the exact "stamp says one model, data holds another" failure the write-time check exists to catch: a deployment upgrading with `YORISHIRO_LOCAL_MODEL` unset now boots onto e5, and every write to a workspace still stamped nomic is refused (loudly, `422`) rather than silently mixed, until `reindex_embeddings` is run against it.
/// A workspace that was created before this deployment's model changed and still carries no stamp (both `embedding_model` and `embedding_dimensions` are `NULL`) inherits the deployment default, so it is also protected: writes go through the stamp that `sync_embedding` sets on the first embed, and the model check compares against that stamp.
/// `docs/configuration.md`'s "Moving a workspace between embedding models" section documents the procedure for every other workspace: change configuration, restart, then reindex.
pub(super) const DEFAULT_MODEL: &LocalModelDef = &MULTILINGUAL_E5_BASE;

/// Where an auto-fetched model lives, scoped to `def` so two models' cached files can never collide or be mismatched with each other.
///
/// Returns `None` when `HOME` does not resolve.
/// The Docker image sets `HOME` for its service user for exactly this reason, but nothing here may assume that holds for every deployment: an operator running the binary directly, under a different supervisor, or in a container image of their own can still reach a user with no home.
/// Nothing invents a fallback directory in that case; the caller degrades and names the two path variables instead, since writing hundreds of megabytes to a guessed location is worse than not writing it.
fn cache_dir(def: &LocalModelDef) -> Option<PathBuf> {
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
    // The pid suffix keeps a server and a worker starting together from writing the same partial file; both verify identical bytes, so whichever renames last is harmless.
    let temp = dir.join(format!(
        "{}.partial.{}",
        artifact.local_name,
        std::process::id()
    ));

    let result = download_verified(&url, &temp, artifact).await;
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

/// How long a `.partial.` file must have gone untouched before a sweep will remove it.
///
/// Long enough that an in-flight download is never a candidate: the sweep only ever looks at files whose mtime has not moved for this long, and a live download writes continuously.
const STALE_PARTIAL_AGE: std::time::Duration = std::time::Duration::from_secs(6 * 60 * 60);

/// Removes abandoned temp files left by earlier killed downloads.
///
/// A killed download leaves `<name>.partial.<pid>`, and the next start has a different pid, so without this nothing ever reuses or removes it: on a deployment whose network keeps dropping mid-fetch, that is hundreds of megabytes of dead bytes per attempt, accumulating forever.
///
/// The age check is what keeps this away from a download that is still running.
/// Two processes starting together (a server and a worker, or `--server-and-worker` alongside a task) each write their own pid-suffixed file, and a sweep that removed a live one would fail the other's rename with `ENOENT`.
/// Requiring [`STALE_PARTIAL_AGE`] of no writes means an in-flight download is never a candidate, since it is writing continuously; anything that old belongs to a process that is gone.
/// Even in the case where that is somehow wrong, the consequence is bounded: the rename fails, that start fails with it, and under a supervisor's restart the next attempt finds the destination already there or fetches it cleanly.
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
async fn download_verified(url: &str, temp: &Path, artifact: &Artifact) -> anyhow::Result<()> {
    use tokio::io::AsyncWriteExt as _;

    let mut response = reqwest::get(url)
        .await
        .map_err(|err| anyhow::anyhow!("failed to fetch {url}: {err}"))?
        .error_for_status()
        .map_err(|err| anyhow::anyhow!("failed to fetch {url}: {err}"))?;

    let mut file = tokio::fs::File::create(temp)
        .await
        .map_err(|err| anyhow::anyhow!("failed to create {}: {err}", temp.display()))?;

    // Streamed chunk by chunk rather than collected with `bytes()`: buffering hundreds of megabytes in memory only to write it straight back out costs the whole model's size in resident memory for no benefit.
    let mut hasher = Sha256::new();
    let mut written: u64 = 0;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|err| anyhow::anyhow!("failed while downloading {url}: {err}"))?
    {
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
    use super::*;

    #[test]
    fn hex_encode_pads_single_digit_bytes() {
        assert_eq!(hex_encode(&[0x00, 0x0f, 0xff, 0xa5]), "000fffa5");
        assert_eq!(hex_encode(&[]), "");
    }

    /// The digests are what the whole mechanism rests on, so a typo in one is worth catching here rather than at a failed startup after a hundreds-of-megabytes download.
    #[test]
    fn digests_are_lowercase_sha256_hex() {
        for def in MODELS {
            for artifact in [&def.model, &def.tokenizer] {
                assert_eq!(
                    artifact.sha256.len(),
                    64,
                    "{} {}",
                    def.short_id,
                    artifact.description
                );
                assert!(
                    artifact
                        .sha256
                        .chars()
                        .all(|c| c.is_ascii_digit() || c.is_ascii_lowercase() && c <= 'f'),
                    "{} {} digest must be lowercase hex",
                    def.short_id,
                    artifact.description
                );
            }
            assert_ne!(def.model.sha256, def.tokenizer.sha256, "{}", def.short_id);
        }
    }

    #[test]
    fn revision_is_a_full_commit_sha() {
        for def in MODELS {
            assert_eq!(def.revision.len(), 40, "{}", def.short_id);
            assert!(
                def.revision.chars().all(|c| c.is_ascii_hexdigit()),
                "{}",
                def.short_id
            );
        }
    }

    /// `short_id` names a filesystem directory (see [`cache_dir`]) and selects a model via `YORISHIRO_LOCAL_MODEL`, so it must be a bare identifier with no path separators or whitespace that would need shell-quoting or filesystem-escaping.
    #[test]
    fn short_ids_are_filesystem_and_env_safe() {
        for def in MODELS {
            assert!(
                def.short_id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.'),
                "{}",
                def.short_id
            );
        }
    }

    /// `MODELS` is the only place `YORISHIRO_LOCAL_MODEL`'s error message and `resolve_local_model` look for a definition, so a duplicate `short_id` would make one of two models unreachable while looking configured.
    #[test]
    fn short_ids_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for def in MODELS {
            assert!(
                seen.insert(def.short_id),
                "duplicate short_id: {}",
                def.short_id
            );
        }
    }

    /// The sweep must remove an abandoned partial while leaving a freshly written one alone, since a live download's temp file looks exactly like an abandoned one apart from its age.
    #[test]
    fn the_sweep_removes_only_partials_old_enough_to_be_abandoned() {
        let dir = std::env::temp_dir().join(format!("yorishiro-sweep-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("cannot create test dir");

        let in_flight = dir.join(format!("{}.partial.999999", NOMIC.tokenizer.local_name));
        let unrelated = dir.join(NOMIC.tokenizer.local_name);
        let other_artifact = dir.join(format!("{}.partial.1", NOMIC.model.local_name));
        for path in [&in_flight, &unrelated, &other_artifact] {
            std::fs::write(path, b"x").expect("cannot write fixture");
        }

        let abandoned = dir.join(format!("{}.partial.12345", NOMIC.tokenizer.local_name));
        std::fs::write(&abandoned, b"x").expect("cannot write fixture");
        // Backdating the mtime is what makes this a test of the age rule rather than of the filename prefix alone.
        let long_ago =
            std::time::SystemTime::now() - STALE_PARTIAL_AGE - std::time::Duration::from_secs(60);
        std::fs::File::open(&abandoned)
            .expect("cannot open fixture")
            .set_modified(long_ago)
            .expect("cannot backdate fixture");

        sweep_stale_partials(&dir, &NOMIC.tokenizer);

        assert!(!abandoned.exists(), "an abandoned partial must be removed");
        assert!(
            in_flight.exists(),
            "a partial still being written must survive"
        );
        assert!(unrelated.exists(), "the real file must never be touched");
        assert!(
            other_artifact.exists(),
            "another artifact's partial is not this sweep's to remove"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Fetches the real tokenizer over the network and checks the whole download-verify-rename sequence, including that a wrong digest is rejected and leaves nothing behind.
    ///
    /// `#[ignore]` because it needs the network: CI runs offline and would fail on it, so it is run deliberately with `cargo test -- --ignored fetches_and_verifies`.
    /// The tokenizer, not the model: it exercises identical code at a fraction of the size.
    #[tokio::test]
    #[ignore = "requires network access to huggingface.co"]
    async fn fetches_and_verifies_a_real_artifact() {
        let dir = std::env::temp_dir().join(format!("yorishiro-fetch-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let path = ensure_file(&dir, &NOMIC, &NOMIC.tokenizer)
            .await
            .expect("fetch failed");
        let bytes = std::fs::read(&path).expect("downloaded file unreadable");
        assert_eq!(bytes.len() as u64, NOMIC.tokenizer.size);
        assert_eq!(hex_encode(&Sha256::digest(&bytes)), NOMIC.tokenizer.sha256);

        // A file already in place is used as-is rather than fetched again, which is what keeps only the first start paying for the download.
        let again = ensure_file(&dir, &NOMIC, &NOMIC.tokenizer)
            .await
            .expect("second call failed");
        assert_eq!(again, path);

        // A cached file of the wrong length must be replaced rather than returned: it passed its digest before some earlier rename, but nothing has looked at it since, and loading it would embed against corrupt bytes with every status still healthy.
        std::fs::write(&path, b"truncated").expect("cannot truncate the cached file");
        let repaired = ensure_file(&dir, &NOMIC, &NOMIC.tokenizer)
            .await
            .expect("a corrupt cached file must be refetched, not returned");
        assert_eq!(repaired, path);
        assert_eq!(
            std::fs::metadata(&path)
                .expect("refetched file missing")
                .len(),
            NOMIC.tokenizer.size,
            "the corrupt cached file should have been replaced by a complete one"
        );
        assert_eq!(
            hex_encode(&Sha256::digest(
                std::fs::read(&path).expect("refetched file unreadable")
            )),
            NOMIC.tokenizer.sha256
        );

        // A digest that does not match the bytes must fail and leave no partial file to be mistaken for a good one later.
        let corrupt = Artifact {
            sha256: "0000000000000000000000000000000000000000000000000000000000000000",
            local_name: "corrupt.json",
            ..NOMIC.tokenizer
        };
        let err = ensure_file(&dir, &NOMIC, &corrupt)
            .await
            .expect_err("a wrong digest must be rejected");
        assert!(err.to_string().contains("SHA256"), "{err}");
        assert!(!dir.join("corrupt.json").exists());
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .expect("cache dir unreadable")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name())
            .filter(|name| name.to_string_lossy().contains(".partial."))
            .collect();
        assert!(
            leftovers.is_empty(),
            "partial files left behind: {leftovers:?}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
