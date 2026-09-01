/// Tests for the rate limiter: guard checks and token accounting.
use std::time::Duration;

use async_trait::async_trait;
use uuid::Uuid;
use yorishiro::error::YorishiroError;
use yorishiro::services::embedding::EmbeddingProvider;
use yorishiro::services::rate_limit::{RateLimiter, charge_search_tokens};

/// Counts tokens as the byte length exactly, so a test can pick a query whose cost is known up front rather than depending on the default estimate's rounding.
struct FixedCostProvider;

#[async_trait]
impl EmbeddingProvider for FixedCostProvider {
    fn dimensions(&self) -> usize {
        1
    }
    fn model_name(&self) -> String {
        "fixed-cost".into()
    }
    fn count_tokens(&self, text: &str) -> u32 {
        text.len() as u32
    }
    async fn embed_batch(&self, _texts: &[&str]) -> Result<Vec<Vec<f32>>, YorishiroError> {
        unimplemented!("not exercised by these tests")
    }
}

#[test]
fn charge_search_tokens_exhausts_the_budget_and_then_rejects() {
    let limiter = RateLimiter::new(10, Duration::from_secs(60));
    let provider = FixedCostProvider;
    let workspace = Uuid::new_v4();

    // "0123456789" costs 10 tokens: exactly the budget, so this call is admitted.
    charge_search_tokens(&limiter, &provider, workspace, "0123456789").unwrap();
    // The budget is now spent; the same workspace's next call is rejected.
    assert!(charge_search_tokens(&limiter, &provider, workspace, "x").is_err());
}

#[test]
fn charge_search_tokens_is_keyed_per_workspace() {
    let limiter = RateLimiter::new(10, Duration::from_secs(60));
    let provider = FixedCostProvider;
    let workspace = Uuid::new_v4();
    let other = Uuid::new_v4();

    charge_search_tokens(&limiter, &provider, workspace, "0123456789").unwrap();
    assert!(charge_search_tokens(&limiter, &provider, workspace, "x").is_err());
    // A different workspace has its own, untouched budget.
    charge_search_tokens(&limiter, &provider, other, "0123456789").unwrap();
}
