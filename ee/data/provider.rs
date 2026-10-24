use crate::YorishiroError;

pub(crate) fn validate_credentials<'a>(
    base_url: &'a str,
    api_key: &str,
) -> Result<&'a str, YorishiroError> {
    if api_key.trim().is_empty() {
        return Err(YorishiroError::ValidationFailed {
            message: "api_key must not be empty".into(),
            details: vec![],
            hint: "remove the configuration instead of storing an empty key".into(),
        });
    }
    let base_url = base_url.trim().trim_end_matches('/');
    if !base_url.starts_with("http://") && !base_url.starts_with("https://") {
        return Err(YorishiroError::ValidationFailed {
            message: "base_url must start with http:// or https://".into(),
            details: vec![],
            hint: "for example https://api.openai.com/v1".into(),
        });
    }
    Ok(base_url)
}
