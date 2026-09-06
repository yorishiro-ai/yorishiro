/// Tests for model fetching: hex encoding, digest validation, and partial file cleanup.
use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;
use yorishiro::services::embedding::model_fetch::{self, Artifact, ensure_file, sweep_stale_partials, STALE_PARTIAL_AGE, MODELS, DEFAULT_MODEL};

#[test]
fn hex_encode_pads_single_digit_bytes() {
    assert_eq!(
        model_fetch::hex_encode(&[0x00, 0x0f, 0xff, 0xa5]),
        "000fffa5"
    );
    assert_eq!(model_fetch::hex_encode(&[]), "");
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
                    .all(|c| c.is_ascii_digit() || (c.is_ascii_lowercase() && c <= 'f')),
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
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("cannot create test dir");

    // "tokenizer.json" and "model.safetensors" are the only file names MODELS ever uses.
    // Hardcoding them rather than reaching for MULTILINGUAL_E5_BASE keeps the test
    // independent of any particular model definition, so a future model swap does not
    // silently change what files this sweep exercises.
    let in_flight = dir.join("tokenizer.json.partial.999999");
    let unrelated = dir.join("tokenizer.json");
    let other_artifact = dir.join("model.safetensors.partial.1");
    for path in [&in_flight, &unrelated, &other_artifact] {
        fs::write(path, b"x").expect("cannot write fixture");
    }

    let abandoned = dir.join("tokenizer.json.partial.12345");
    fs::write(&abandoned, b"x").expect("cannot write fixture");
    // Backdating the mtime is what makes this a test of the age rule rather than of the filename prefix alone.
    let long_ago =
        std::time::SystemTime::now() - STALE_PARTIAL_AGE - std::time::Duration::from_secs(60);
    fs::File::open(&abandoned)
        .expect("cannot open fixture")
        .set_modified(long_ago)
        .expect("cannot backdate fixture");

    sweep_stale_partials(
        &dir,
        &Artifact {
            remote_path: "tokenizer.json",
            local_name: "tokenizer.json",
            sha256: "0".repeat(64).as_str(),
            size: 0,
            description: "tokenizer",
        },
    );

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

    let _ = fs::remove_dir_all(&dir);
}

/// Fetches the real tokenizer over the network and checks the whole download-verify-rename sequence, including that a wrong digest is rejected and leaves nothing behind.
///
/// `#[ignore]` because it needs the network: CI runs offline and would fail on it, so it is run deliberately with `cargo test -- --ignored fetches_and_verifies`.
/// The tokenizer, not the model: it exercises identical code at a fraction of the size.
#[tokio::test]
#[ignore = "requires network access to huggingface.co"]
async fn fetches_and_verifies_a_real_artifact() {
    let dir = std::env::temp_dir().join(format!("yorishiro-fetch-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);

    // Use DEFAULT_MODEL directly instead of hardcoding its fields, so a future model
    // change automatically updates what this test exercises.
    let path = ensure_file(&dir, DEFAULT_MODEL, &DEFAULT_MODEL.tokenizer)
        .await
        .expect("fetch failed");
    let bytes = fs::read(&path).expect("downloaded file unreadable");
    assert_eq!(bytes.len() as u64, DEFAULT_MODEL.tokenizer.size);
    assert_eq!(
        model_fetch::hex_encode(&Sha256::digest(&bytes)),
        DEFAULT_MODEL.tokenizer.sha256
    );

    // A file already in place is used as-is rather than fetched again, which is what keeps only the first start paying for the download.
    let again = ensure_file(&dir, DEFAULT_MODEL, &DEFAULT_MODEL.tokenizer)
        .await
        .expect("second call failed");
    assert_eq!(again, path);

    // A cached file of the wrong length must be replaced rather than returned: loading it
    // would embed against corrupt bytes with every status still healthy.
    fs::write(&path, b"truncated").expect("cannot truncate the cached file");
    let repaired = ensure_file(&dir, DEFAULT_MODEL, &DEFAULT_MODEL.tokenizer)
        .await
        .expect("a corrupt cached file must be refetched, not returned");
    assert_eq!(repaired, path);
    assert_eq!(
        fs::metadata(&path)
            .expect("refetched file missing")
            .len(),
        DEFAULT_MODEL.tokenizer.size,
        "the corrupt cached file should have been replaced by a complete one"
    );
    assert_eq!(
        model_fetch::hex_encode(&Sha256::digest(
            fs::read(&path).expect("refetched file unreadable")
        )),
        DEFAULT_MODEL.tokenizer.sha256
    );

    // A digest that does not match the bytes must fail and leave no partial file to be mistaken for a good one later.
    let corrupt = Artifact {
        sha256: "0000000000000000000000000000000000000000000000000000000000000000",
        local_name: "corrupt.json",
        ..DEFAULT_MODEL.tokenizer
    };
    let err = ensure_file(&dir, DEFAULT_MODEL, &corrupt)
        .await
        .expect_err("a wrong digest must be rejected");
    assert!(err.to_string().contains("SHA256"), "{err}");
    assert!(!dir.join("corrupt.json").exists());
    let leftovers: Vec<_> = fs::read_dir(&dir)
        .expect("cache dir unreadable")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name())
        .filter(|name| name.to_string_lossy().contains(".partial."))
        .collect();
    assert!(
        leftovers.is_empty(),
        "partial files left behind: {leftovers:?}"
    );

    let _ = fs::remove_dir_all(&dir);
}
