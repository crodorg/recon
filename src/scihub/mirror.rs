//! The Sci-Hub live-domain mirror: a probe-curated cache of which hosts actually
//! work right now.
//!
//! Sci-Hub rotates domains constantly under legal pressure (e.g. `sci-hub.se` was
//! DNS-blocked in Jan 2026), and every third-party "current domains" page is
//! itself unreliable and moves. So we don't *trust* a list — we **probe** one: ask
//! each candidate host for a paper we know predates the 2021 corpus freeze and see
//! if it coughs up a real PDF. The ones that do are live; the rest are dropped. The
//! list curates itself.
//!
//! Seed = a compiled starting guess ∪ the operator's `~/.config/research/scihub.conf`
//! (the real refresh lever when domains move). Cache lives at
//! `~/.local/share/research/scihub-domains.json` and auto-refreshes past a TTL.

use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use futures::future::join_all;
use serde::{Deserialize, Serialize};

use crate::http;
use crate::scihub::fetch::extract_pdf_url;

/// Compiled seed of candidate Sci-Hub hosts — a *starting guess*, never
/// authoritative. The runtime probe decides what's actually live; the operator's
/// config file is how new domains get added when these rot.
const SEED_DOMAINS: &[&str] = &[
    "sci-hub.se",
    "sci-hub.st",
    "sci-hub.ru",
    "sci-hub.al",
    "sci-hub.ee",
    "sci-hub.ren",
];

/// Canary: Watson & Crick, "Molecular Structure of Nucleic Acids" (Nature, 1953).
/// Foundational, decades pre-freeze → guaranteed indexed. A host that returns a
/// real embedded PDF for this is live; this also self-tests the PDF extractor.
const CANARY_DOI: &str = "10.1038/171737a0";

/// Auto-refresh the mirror when the cache is older than this many hours.
const TTL_HOURS: i64 = 24;

/// Per-probe timeout (seconds) — a dead/parked host should fail fast.
const PROBE_TIMEOUT_SECS: u64 = 15;

/// One host's probe result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomainStatus {
    pub host: String,
    /// "live" | "dead".
    pub status: String,
    pub last_checked: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
}

/// The on-disk mirror: when it was last probed and every candidate's status.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct DomainCache {
    pub updated_at: String,
    pub domains: Vec<DomainStatus>,
}

impl DomainCache {
    /// Live hosts, fastest first.
    pub fn live(&self) -> Vec<DomainStatus> {
        let mut live: Vec<DomainStatus> = self
            .domains
            .iter()
            .filter(|d| d.status == "live")
            .cloned()
            .collect();
        live.sort_by_key(|d| d.latency_ms.unwrap_or(u64::MAX));
        live
    }
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// `~/.local/share/research/scihub-domains.json` — same state root as run dirs.
fn cache_path() -> PathBuf {
    home().join(".local/share/research/scihub-domains.json")
}

/// `~/.config/research/scihub.conf` — same config dir as `trust.conf`.
fn config_path() -> PathBuf {
    home().join(".config/research/scihub.conf")
}

/// Seed hosts = operator config (one host per line, `#` comments) ∪ compiled
/// defaults, deduped, config first. Bare hosts only — scheme/trailing slash stripped.
fn load_seed() -> Vec<String> {
    let mut hosts: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    if let Ok(text) = std::fs::read_to_string(config_path()) {
        for line in text.lines() {
            let h = sanitize_host(line.split('#').next().unwrap_or(""));
            if !h.is_empty() && seen.insert(h.clone()) {
                hosts.push(h);
            }
        }
    }
    for d in SEED_DOMAINS {
        if seen.insert(d.to_string()) {
            hosts.push(d.to_string());
        }
    }
    hosts
}

/// Strip scheme, leading/trailing slashes, and whitespace down to a bare host.
fn sanitize_host(raw: &str) -> String {
    raw.trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_matches('/')
        .trim()
        .to_ascii_lowercase()
}

/// Read the cache, or `None` if absent/corrupt.
pub fn read_cache() -> Option<DomainCache> {
    let text = std::fs::read_to_string(cache_path()).ok()?;
    serde_json::from_str(&text).ok()
}

fn write_cache(cache: &DomainCache) -> Result<()> {
    let path = cache_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create {}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(cache).context("serialize domain cache")?;
    std::fs::write(&path, json).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

/// Probe one host: fetch the canary page and try to extract a real PDF URL.
/// Network or parse failure ⇒ dead.
async fn probe_domain(client: &reqwest::Client, host: &str) -> DomainStatus {
    let url = format!("https://{host}/{CANARY_DOI}");
    let start = Instant::now();
    let live = match http::get_text(client, &url).await {
        Ok(html) => extract_pdf_url(&html, host).is_some(),
        Err(_) => false,
    };
    let latency = start.elapsed().as_millis() as u64;
    DomainStatus {
        host: host.to_string(),
        status: if live { "live" } else { "dead" }.to_string(),
        last_checked: crate::model::now_iso(),
        latency_ms: live.then_some(latency),
    }
}

/// Re-probe every seed/config host concurrently and rewrite the cache.
pub async fn refresh() -> Result<DomainCache> {
    let client = http::client_timeout(PROBE_TIMEOUT_SECS);
    let seed = load_seed();
    let statuses: Vec<DomainStatus> =
        join_all(seed.iter().map(|h| probe_domain(&client, h))).await;
    let cache = DomainCache {
        updated_at: crate::model::now_iso(),
        domains: statuses,
    };
    write_cache(&cache)?;
    Ok(cache)
}

/// True if the cache timestamp is older than the TTL (or unparseable).
fn is_stale(updated_at: &str) -> bool {
    match DateTime::parse_from_rfc3339(updated_at) {
        Ok(t) => Utc::now()
            .signed_duration_since(t.with_timezone(&Utc))
            .num_hours()
            >= TTL_HOURS,
        Err(_) => true,
    }
}

/// Live domains (fastest first), refreshing when forced, or when the cache is
/// missing/stale. This is the self-refreshing entry point the fetcher calls.
pub async fn live_domains(force: bool) -> Result<Vec<DomainStatus>> {
    if !force {
        if let Some(cache) = read_cache() {
            if !is_stale(&cache.updated_at) {
                return Ok(cache.live());
            }
        }
    }
    Ok(refresh().await?.live())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_scheme_and_slashes() {
        assert_eq!(sanitize_host(" https://Sci-Hub.ST/ "), "sci-hub.st");
        assert_eq!(sanitize_host("sci-hub.ru"), "sci-hub.ru");
        assert_eq!(sanitize_host("http://sci-hub.se///"), "sci-hub.se");
    }

    #[test]
    fn fresh_cache_is_not_stale_and_old_one_is() {
        let now = Utc::now().to_rfc3339();
        assert!(!is_stale(&now));
        let old = (Utc::now() - chrono::Duration::hours(TTL_HOURS + 1)).to_rfc3339();
        assert!(is_stale(&old));
        assert!(is_stale("not-a-date"));
    }

    #[test]
    fn live_sorts_fastest_first_and_drops_dead() {
        let cache = DomainCache {
            updated_at: Utc::now().to_rfc3339(),
            domains: vec![
                DomainStatus { host: "slow".into(), status: "live".into(), last_checked: "".into(), latency_ms: Some(900) },
                DomainStatus { host: "dead".into(), status: "dead".into(), last_checked: "".into(), latency_ms: None },
                DomainStatus { host: "fast".into(), status: "live".into(), last_checked: "".into(), latency_ms: Some(100) },
            ],
        };
        let live = cache.live();
        assert_eq!(live.iter().map(|d| d.host.as_str()).collect::<Vec<_>>(), vec!["fast", "slow"]);
    }
}
