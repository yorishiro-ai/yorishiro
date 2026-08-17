//! Splitting long work into acknowledgeable pieces (FR-10, §7.5).
//!
//! A worker that processes a whole job and then acknowledges loses everything if it dies at 99%.
//! Splitting the job first bounds that loss to one chunk, and lets whatever runs the chunks be
//! stateless: a node leaving is not a failed job, it is a handful of chunks nobody
//! acknowledged.
//!
//! # What is here, and what is not
//!
//! The **framework** is here: splitting, and the record of which chunks were acknowledged. The
//! **reassignment** is not, and cannot be: returning an unacknowledged chunk to the pool is a
//! visibility timeout, which is a property of a queue that ships work between processes.
//! `LocalQueue` runs tasks on this runtime, where a lost chunk and a lost process are the same
//! event. A distributed driver brings its own timeout and reuses everything here:
//! [`ChunkProgress::outstanding`] is exactly the list it hands back.

use crate::error::YorishiroError;

/// The window §7.5 asks for.
/// Small enough that losing one is cheap, large enough that the per-chunk overhead is not the cost.
pub const MIN_CHUNK_TOKENS: usize = 1_000;
pub const MAX_CHUNK_TOKENS: usize = 2_000;

/// One piece of a job, identified so an acknowledgement can name it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    /// Position in the original sequence.
    /// What an acknowledgement refers to, and what tells a reader which piece is missing.
    pub index: usize,
    pub text: String,
}

/// Splits text into chunks of roughly `target` tokens, breaking at whitespace.
///
/// Token counts are approximated by whitespace-separated words.
/// The exact figure belongs to the tokenizer of whichever model will embed this, which this module does not know, and the window is a range precisely because it does not have to be exact.
///
/// Never splits mid-word: a chunk ending halfway through one would embed a fragment that means something else, and the boundary is arbitrary anyway.
pub fn split(text: &str, target: usize) -> Result<Vec<Chunk>, YorishiroError> {
    if target == 0 {
        return Err(YorishiroError::ValidationFailed {
            message: "chunk target must be positive".into(),
            details: vec![],
            hint: format!("use something between {MIN_CHUNK_TOKENS} and {MAX_CHUNK_TOKENS}"),
        });
    }

    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return Ok(Vec::new());
    }

    Ok(words
        .chunks(target)
        .enumerate()
        .map(|(index, group)| Chunk {
            index,
            text: group.join(" "),
        })
        .collect())
}

/// Which chunks of a job have been acknowledged.
///
/// Deliberately not a count: a worker that acknowledges the same chunk twice must not make the job look finished, and a reader asking "what is missing" wants the indices, not the arithmetic.
#[derive(Debug)]
pub struct ChunkProgress {
    total: usize,
    acknowledged: std::collections::BTreeSet<usize>,
}

impl ChunkProgress {
    pub fn new(total: usize) -> Self {
        Self {
            total,
            acknowledged: std::collections::BTreeSet::new(),
        }
    }

    /// Records that a chunk completed.
    /// Idempotent: a redelivered chunk acknowledged twice is the normal case for an at-least-once queue, not an error.
    pub fn acknowledge(&mut self, index: usize) {
        if index < self.total {
            self.acknowledged.insert(index);
        }
    }

    /// The chunks nobody has acknowledged.
    /// What a distributed driver returns to the pool once its visibility timeout expires.
    pub fn outstanding(&self) -> Vec<usize> {
        (0..self.total)
            .filter(|i| !self.acknowledged.contains(i))
            .collect()
    }

    pub fn is_complete(&self) -> bool {
        self.acknowledged.len() == self.total
    }
}

#[cfg(test)]
#[path = "../../tests/services/chunking.rs"]
mod tests;
