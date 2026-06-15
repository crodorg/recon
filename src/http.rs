//! Shared HTTP client + JSON fetch helper for all connectors.

use anyhow::Context;

pub const USER_AGENT: &str = "recon-cli/0.1 (+terminal deep-research)";

/// Build the shared reqwest client (fixed UA, 20s timeout).
pub fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .unwrap()
}

/// Build a client with a custom timeout (seconds). PDF downloads can be a few MB
/// and a domain probe should fail fast — neither fits the shared 20s default.
/// Used only by the optional `scihub` feature.
#[cfg(feature = "scihub")]
pub fn client_timeout(secs: u64) -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(std::time::Duration::from_secs(secs))
        .build()
        .unwrap()
}

/// GET a URL and parse the body as JSON.
pub async fn get_json(client: &reqwest::Client, url: &str) -> anyhow::Result<serde_json::Value> {
    let resp = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?
        .error_for_status()
        .with_context(|| format!("HTTP status for {url}"))?;
    let value = resp
        .json::<serde_json::Value>()
        .await
        .with_context(|| format!("decode JSON from {url}"))?;
    Ok(value)
}

/// GET a URL and return the body as text (HTML pages, etc.).
/// Used only by the optional `scihub` feature.
#[cfg(feature = "scihub")]
pub async fn get_text(client: &reqwest::Client, url: &str) -> anyhow::Result<String> {
    let resp = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?
        .error_for_status()
        .with_context(|| format!("HTTP status for {url}"))?;
    let text = resp
        .text()
        .await
        .with_context(|| format!("read body of {url}"))?;
    Ok(text)
}
