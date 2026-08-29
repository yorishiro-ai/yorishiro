//! First-use fetch of the local ONNX provider's model and tokenizer.
//!
//! `YORISHIRO_EMBEDDING_PROVIDER=local` needs an ONNX model and its tokenizer, neither of which is in the repository: the model alone is about 522 MiB, which does not belong in git.
//! Rather than making an operator fetch them by hand, the default path is fetched on first use and verified against a hardcoded digest.
//!
//! Only the *default* path is ever fetched.
//! An operator who sets `YORISHIRO_ONNX_MODEL_PATH` or `YORISHIRO_ONNX_TOKENIZER_PATH` has told us where the file already is, so a wrong path there fails loudly instead of quietly downloading half a gigabyte somewhere they did not ask for; see [`super::build_local_onnx_provider`].

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// The model revision this deployment pins.
///
/// A tag or `main` would let the bytes behind these digests change under us, turning a legitimate upstream update into a verification failure that looks like corruption.
const REVISION: &str = "e9b6763023c676ca8431644204f50c2b100d9aab";

const REPO: &str = "nomic-ai/nomic-embed-text-v1.5";

/// One file to fetch, with everything needed to verify it.
struct Artifact {
    /// Path within the model repository, at [`REVISION`].
    remote_path: &'static str,
    /// Name under the cache directory, matching the default file names in [`super::build_local_onnx_provider`].
    local_name: &'static str,
    /// SHA256 of the file's bytes.
    ///
    /// Taken by downloading each file at [`REVISION`] and running `sha256sum` on it, never transcribed from a response header.
    /// The headers cannot be trusted for this. `model.onnx` answers with two different 64-hex values (`x-linked-etag` on the 302 and `etag` on the 200), and nothing in the response says which is the content digest; it is the 302's. `tokenizer.json`'s ETag matches neither, being a 40-hex Git blob SHA1, since it is small enough not to go through LFS.
    /// Updating [`REVISION`] means re-measuring these the same way.
    sha256: &'static str,
    /// Expected length in bytes, checked before hashing.
    ///
    /// A truncated download is the ordinary failure at this size, and a length comparison rejects one without reading 522 MiB back through SHA256 first.
    size: u64,
    /// What to call this in a log line an operator reads.
    description: &'static str,
}

const MODEL: Artifact = Artifact {
    remote_path: "onnx/model.onnx",
    local_name: "model.onnx",
    sha256: "147d5aa88c2101237358e17796cf3a227cead1ec304ec34b465bb08e9d952965",
    size: 547_310_275,
    description: "ONNX model",
};

const TOKENIZER: Artifact = Artifact {
    remote_path: "tokenizer.json",
    local_name: "tokenizer.json",
    sha256: "d241a60d5e8f04cc1b2b3e9ef7a4921b27bf526d9f6050ab90f9267a1f9e5c66",
    size: 711_396,
    description: "tokenizer",
};

/// Where an auto-fetched model lives.
///
/// Returns `None` when `HOME` does not resolve, which is not hypothetical: the Docker image's user is created `--no-create-home`.
/// Nothing invents a fallback directory in that case; the caller degrades and names the two path variables instead, since writing half a gigabyte to a guessed location is worse than not writing it.
fn cache_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    if home.is_empty() {
        return None;
    }
    Some(PathBuf::from(home).join(".cache/yorishiro/models"))
}

/// The model and tokenizer paths to load from, fetching either one first if it is absent.
///
/// `Ok(None)` means the destination could not be resolved at all (no `HOME`), so the caller degrades rather than failing.
/// `Err` means a fetch was attempted and failed, which fails the boot; see [`super::build_local_onnx_provider`] for why those two outcomes differ.
pub(super) async fn ensure_model_files() -> anyhow::Result<Option<(PathBuf, PathBuf)>> {
    let Some(dir) = cache_dir() else {
        return Ok(None);
    };

    let model = ensure_file(&dir, &MODEL).await?;
    let tokenizer = ensure_file(&dir, &TOKENIZER).await?;
    Ok(Some((model, tokenizer)))
}

/// Fetches one artifact into `dir` unless it is already there, and returns its path.
async fn ensure_file(dir: &Path, artifact: &Artifact) -> anyhow::Result<PathBuf> {
    let destination = dir.join(artifact.local_name);
    if destination.exists() {
        // The size is checked, and deliberately not the digest. Do not "fix" this into a re-verification: the reasons it is not one are the whole point.
        //
        // The digest guards the wire, not the disk. This destination only ever receives bytes that already passed both length and SHA256, moved in by an atomic rename within one filesystem, so the mechanism cannot itself produce a bad file here.
        // Getting one needs an outside writer or disk corruption, and corruption that mangles an ONNX protobuf fails loudly in `LocalOnnxProvider::load` regardless.
        // The quiet case, a valid but different model swapped in, needs someone holding the service user's write access, and they could as easily set `YORISHIRO_ONNX_MODEL_PATH` or replace the `models/` files, neither of which is checked at all.
        //
        // Re-hashing only this tier would also invert the design: files an operator placed themselves are deliberately unverified, since a custom model cannot match a digest pinned to nomic's, so re-checking at read time the one tier that was already verified at write time, at 522 MiB on every single start forever, would spend the cost exactly where it buys least.
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
        "https://huggingface.co/{REPO}/resolve/{REVISION}/{}",
        artifact.remote_path
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
/// A killed download leaves `<name>.partial.<pid>`, and the next start has a different pid, so without this nothing ever reuses or removes it: on a deployment whose network keeps dropping mid-fetch, that is 522 MiB of dead bytes per attempt, accumulating forever.
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

    // Streamed chunk by chunk rather than collected with `bytes()`: buffering 522 MiB in memory only to write it straight back out costs the whole model's size in resident memory for no benefit.
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

    /// The digests are what the whole mechanism rests on, so a typo in one is worth catching here rather than at a failed startup after a 522 MiB download.
    #[test]
    fn digests_are_lowercase_sha256_hex() {
        for artifact in [&MODEL, &TOKENIZER] {
            assert_eq!(artifact.sha256.len(), 64, "{}", artifact.description);
            assert!(
                artifact
                    .sha256
                    .chars()
                    .all(|c| c.is_ascii_digit() || c.is_ascii_lowercase() && c <= 'f'),
                "{} digest must be lowercase hex",
                artifact.description
            );
        }
        assert_ne!(MODEL.sha256, TOKENIZER.sha256);
    }

    #[test]
    fn revision_is_a_full_commit_sha() {
        assert_eq!(REVISION.len(), 40);
        assert!(REVISION.chars().all(|c| c.is_ascii_hexdigit()));
    }

    /// The sweep must remove an abandoned partial while leaving a freshly written one alone, since a live download's temp file looks exactly like an abandoned one apart from its age.
    #[test]
    fn the_sweep_removes_only_partials_old_enough_to_be_abandoned() {
        let dir = std::env::temp_dir().join(format!("yorishiro-sweep-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("cannot create test dir");

        let in_flight = dir.join(format!("{}.partial.999999", TOKENIZER.local_name));
        let unrelated = dir.join(TOKENIZER.local_name);
        let other_artifact = dir.join(format!("{}.partial.1", MODEL.local_name));
        for path in [&in_flight, &unrelated, &other_artifact] {
            std::fs::write(path, b"x").expect("cannot write fixture");
        }

        let abandoned = dir.join(format!("{}.partial.12345", TOKENIZER.local_name));
        std::fs::write(&abandoned, b"x").expect("cannot write fixture");
        // Backdating the mtime is what makes this a test of the age rule rather than of the filename prefix alone.
        let long_ago =
            std::time::SystemTime::now() - STALE_PARTIAL_AGE - std::time::Duration::from_secs(60);
        std::fs::File::open(&abandoned)
            .expect("cannot open fixture")
            .set_modified(long_ago)
            .expect("cannot backdate fixture");

        sweep_stale_partials(&dir, &TOKENIZER);

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
    /// The tokenizer, not the model: it exercises identical code at 711 KiB instead of 522 MiB.
    #[tokio::test]
    #[ignore = "requires network access to huggingface.co"]
    async fn fetches_and_verifies_a_real_artifact() {
        let dir = std::env::temp_dir().join(format!("yorishiro-fetch-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let path = ensure_file(&dir, &TOKENIZER).await.expect("fetch failed");
        let bytes = std::fs::read(&path).expect("downloaded file unreadable");
        assert_eq!(bytes.len() as u64, TOKENIZER.size);
        assert_eq!(hex_encode(&Sha256::digest(&bytes)), TOKENIZER.sha256);

        // A file already in place is used as-is rather than fetched again, which is what keeps only the first start paying for the download.
        let again = ensure_file(&dir, &TOKENIZER)
            .await
            .expect("second call failed");
        assert_eq!(again, path);

        // A cached file of the wrong length must be replaced rather than returned: it passed its digest before some earlier rename, but nothing has looked at it since, and loading it would embed against corrupt bytes with every status still healthy.
        std::fs::write(&path, b"truncated").expect("cannot truncate the cached file");
        let repaired = ensure_file(&dir, &TOKENIZER)
            .await
            .expect("a corrupt cached file must be refetched, not returned");
        assert_eq!(repaired, path);
        assert_eq!(
            std::fs::metadata(&path)
                .expect("refetched file missing")
                .len(),
            TOKENIZER.size,
            "the corrupt cached file should have been replaced by a complete one"
        );
        assert_eq!(
            hex_encode(&Sha256::digest(
                std::fs::read(&path).expect("refetched file unreadable")
            )),
            TOKENIZER.sha256
        );

        // A digest that does not match the bytes must fail and leave no partial file to be mistaken for a good one later.
        let corrupt = Artifact {
            sha256: "0000000000000000000000000000000000000000000000000000000000000000",
            local_name: "corrupt.json",
            ..TOKENIZER
        };
        let err = ensure_file(&dir, &corrupt)
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
