use crate::services::chunking::{ChunkProgress, MAX_CHUNK_TOKENS, MIN_CHUNK_TOKENS, split};

#[test]
fn splits_at_the_target_and_never_mid_word() {
    let text = (0..250)
        .map(|i| format!("word{i}"))
        .collect::<Vec<_>>()
        .join(" ");
    let chunks = split(&text, 100).unwrap();

    assert_eq!(chunks.len(), 3, "250 words at 100 per chunk");
    assert_eq!(chunks[0].index, 0);
    assert_eq!(chunks[2].index, 2);
    assert_eq!(
        chunks[2].text.split_whitespace().count(),
        50,
        "the last chunk is the remainder"
    );

    let rejoined = chunks
        .iter()
        .map(|c| c.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    assert_eq!(rejoined, text, "nothing lost and nothing split mid-word");
}

#[test]
fn empty_text_is_no_chunks_rather_than_one_empty_chunk() {
    assert!(split("", 100).unwrap().is_empty());
    assert!(split("   \n  ", 100).unwrap().is_empty());
}

#[test]
fn a_zero_target_is_refused_rather_than_looping() {
    assert!(split("some text", 0).is_err());
}

#[test]
fn the_window_is_the_one_the_spec_asks_for() {
    assert_eq!(MIN_CHUNK_TOKENS, 1_000);
    assert_eq!(MAX_CHUNK_TOKENS, 2_000);
}

/// The point of tracking indices rather than a count: an at-least-once queue redelivers, and a chunk acknowledged twice must not make a job of three look finished.
#[test]
fn a_redelivered_chunk_does_not_complete_the_job() {
    let mut progress = ChunkProgress::new(3);
    progress.acknowledge(0);
    progress.acknowledge(0);
    progress.acknowledge(0);

    assert!(!progress.is_complete(), "one chunk of three is not three");
    assert_eq!(progress.outstanding(), vec![1, 2]);

    progress.acknowledge(1);
    progress.acknowledge(2);
    assert!(progress.is_complete());
    assert!(progress.outstanding().is_empty());
}

/// What a distributed driver hands back to the pool when its visibility timeout expires: the chunks nobody acknowledged, and only those.
#[test]
fn outstanding_names_exactly_what_was_never_acknowledged() {
    let mut progress = ChunkProgress::new(5);
    progress.acknowledge(0);
    progress.acknowledge(3);
    assert_eq!(progress.outstanding(), vec![1, 2, 4]);
}

/// An index past the end is a bug in the caller, not a reason to make the job uncompletable.
#[test]
fn an_out_of_range_acknowledgement_is_ignored() {
    let mut progress = ChunkProgress::new(2);
    progress.acknowledge(99);
    assert_eq!(progress.outstanding(), vec![0, 1]);
    progress.acknowledge(0);
    progress.acknowledge(1);
    assert!(progress.is_complete());
}
