use crate::YorishiroError;

pub(crate) fn check_client() -> Result<reqwest::Client, YorishiroError> {
    use crate::error::ResultExt;
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .internal()
}

pub(crate) fn check_error(base_url: &str, error: YorishiroError) -> YorishiroError {
    match error {
        YorishiroError::Internal(_) => YorishiroError::ProviderUnreachable {
            url: base_url.into(),
            message: "provider did not return a usable response for the configured model".into(),
        },
        error => error,
    }
}

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
    if !(base_url.starts_with("http://") || base_url.starts_with("https://"))
        || !reqwest::Url::parse(base_url).is_ok_and(|url| {
            url.host().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none()
        })
    {
        return Err(YorishiroError::ValidationFailed {
            message: "base_url must be an HTTP or HTTPS URL with a host and no credentials, query, or fragment".into(),
            details: vec![],
            hint: "for example https://api.openai.com/v1".into(),
        });
    }
    Ok(base_url)
}
