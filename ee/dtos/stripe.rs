use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct VerifiedStripeEvent {
    pub(crate) id: String,
    #[serde(rename = "type")]
    pub(crate) event_type: String,
    pub(crate) created: i64,
    pub(crate) data: StripeEventData,
}

#[derive(Debug, Deserialize)]
pub(crate) struct StripeEventData {
    pub(crate) object: serde_json::Value,
}
