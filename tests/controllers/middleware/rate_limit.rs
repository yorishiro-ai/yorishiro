/// Tests for the rate limiter: guard checks and token accounting.
use std::time::Duration;

use uuid::Uuid;
use yorishiro::controllers::middleware::rate_limit::{RateLimiter, charge_search_tokens};

/// The server holds no tokenizer, so a query costs one token per four bytes, rounded up: forty bytes cost exactly ten tokens.
const TEN_TOKENS: &str = "0123456789012345678901234567890123456789";

#[test]
fn charge_search_tokens_is_keyed_per_workspace() {
    let limiter = RateLimiter::new(10, Duration::from_secs(60));
    let workspace = Uuid::new_v4();
    let other = Uuid::new_v4();

    charge_search_tokens(&limiter, workspace, TEN_TOKENS).unwrap();
    assert!(charge_search_tokens(&limiter, workspace, "x").is_err());
    // A different workspace has its own, untouched budget.
    charge_search_tokens(&limiter, other, TEN_TOKENS).unwrap();
}
