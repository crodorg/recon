//! Polymarket prediction-market connector (least-critical source; best-effort).
//!
//! Polymarket's public Gamma API (`https://gamma-api.polymarket.com/markets`)
//! has NO full-text search: a `?query=` param is silently ignored and the
//! endpoint hard-caps `limit` at 100 per request. So we page through active,
//! open markets in batches of 100, filter client-side by case-insensitive
//! match of the query terms against the market question + description, sort the
//! hits by (numeric) USDC volume descending, and return the top `limit`.
//!
//! Because this is the lowest-value source, every failure is swallowed: a bad
//! page, a non-JSON body, or a missing field never aborts the run — we return
//! whatever we managed to gather (possibly an empty Vec).

use crate::http::get_json;
use crate::model::{Candidate, SourceType};
use reqwest::Client;
use serde_json::{json, Value};

const GAMMA_MARKETS: &str = "https://gamma-api.polymarket.com/markets";

/// Gamma caps `limit` at 100 regardless of what we ask for.
const PAGE_SIZE: usize = 100;

/// Upper bound on markets pulled per search. Keeps the connector responsive and
/// polite (gamma is unauthenticated): at most MAX_PAGES * PAGE_SIZE markets are
/// fetched and scanned client-side.
const MAX_PAGES: usize = 8;

pub async fn search(client: &Client, query: &str, limit: usize) -> anyhow::Result<Vec<Candidate>> {
    if limit == 0 {
        return Ok(Vec::new());
    }

    // Lowercased query terms; an empty query matches everything.
    let terms: Vec<String> = query
        .split_whitespace()
        .map(|t| t.to_ascii_lowercase())
        .collect();

    let mut matches: Vec<MarketRow> = Vec::new();

    for page in 0..MAX_PAGES {
        let offset = page * PAGE_SIZE;
        let url = format!(
            "{GAMMA_MARKETS}?limit={PAGE_SIZE}&offset={offset}&active=true&closed=false&archived=false"
        );

        // Best-effort: a failed/garbage page ends pagination but keeps prior hits.
        let body = match get_json(client, &url).await {
            Ok(v) => v,
            Err(_) => break,
        };
        let arr = match body.as_array() {
            Some(a) if !a.is_empty() => a,
            // Empty array => no more pages. Non-array => unexpected shape; stop.
            _ => break,
        };

        let returned = arr.len();
        for m in arr {
            if market_matches(m, &terms) {
                matches.push(MarketRow::from_value(m));
            }
        }

        // Short page means we have reached the end of the result set.
        if returned < PAGE_SIZE {
            break;
        }
    }

    // Highest-volume markets first (volume is the closest proxy to relevance).
    matches.sort_by(|a, b| {
        b.volume
            .partial_cmp(&a.volume)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    matches.truncate(limit);

    Ok(matches.into_iter().map(MarketRow::into_candidate).collect())
}

/// A market matches when EVERY query term appears (case-insensitive) in the
/// question or description. An empty term list matches every market.
fn market_matches(m: &Value, terms: &[String]) -> bool {
    if terms.is_empty() {
        return true;
    }
    let haystack = format!(
        "{} {}",
        str_field(m, "question"),
        str_field(m, "description")
    )
    .to_ascii_lowercase();
    terms.iter().all(|t| haystack.contains(t.as_str()))
}

/// Extracted, normalized view of a gamma market row.
struct MarketRow {
    event_slug: Option<String>,
    market_slug: Option<String>,
    question: String,
    description: String,
    end_date: Option<String>,
    created_at: Option<String>,
    outcomes: Value,
    outcome_prices: Value,
    volume: f64,
    liquidity: Value,
    active: bool,
}

impl MarketRow {
    fn from_value(m: &Value) -> Self {
        let event_slug = m
            .get("events")
            .and_then(Value::as_array)
            .and_then(|evs| evs.first())
            .and_then(|ev| ev.get("slug"))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string);

        let market_slug = m
            .get("slug")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string);

        MarketRow {
            event_slug,
            market_slug,
            question: str_field(m, "question"),
            description: str_field(m, "description"),
            end_date: opt_str_field(m, "endDate").or_else(|| opt_str_field(m, "endDateIso")),
            created_at: opt_str_field(m, "createdAt").or_else(|| opt_str_field(m, "startDate")),
            // gamma encodes these as JSON-in-a-string; decode to real arrays.
            outcomes: decode_json_string(m.get("outcomes")),
            outcome_prices: decode_json_string(m.get("outcomePrices")),
            volume: num_field(m, "volume"),
            liquidity: number_or_null(m.get("liquidity")),
            active: m.get("active").and_then(Value::as_bool).unwrap_or(false),
        }
    }

    fn into_candidate(self) -> Candidate {
        // Canonical, browseable URL. Prefer the event page; fall back to the
        // market slug under /event/, then to the bare gamma host.
        let raw_url = match (&self.event_slug, &self.market_slug) {
            (Some(ev), _) => format!("https://polymarket.com/event/{ev}"),
            (None, Some(mk)) => format!("https://polymarket.com/event/{mk}"),
            (None, None) => "https://polymarket.com".to_string(),
        };

        let date = self.end_date.or(self.created_at);

        let extra = json!({
            "outcomes": self.outcomes,
            "outcome_prices": self.outcome_prices,
            "volume": self.volume,
            "liquidity": self.liquidity,
            "active": self.active,
        });

        Candidate {
            raw_url,
            title: self.question,
            snippet: self.description,
            date,
            source_type: SourceType::Web,
            origin: "polymarket".to_string(),
            extra,
        }
    }
}

/// Read a string field, defaulting to "".
fn str_field(m: &Value, key: &str) -> String {
    m.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

/// Read a non-empty string field as Option.
fn opt_str_field(m: &Value, key: &str) -> Option<String> {
    m.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Parse a numeric field that gamma may send as either a JSON number or a
/// stringified number (e.g. `"volume":"821804.10"`). Missing/unparseable => 0.0.
fn num_field(m: &Value, key: &str) -> f64 {
    match m.get(key) {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0),
        Some(Value::String(s)) => s.trim().parse::<f64>().unwrap_or(0.0),
        _ => 0.0,
    }
}

/// Normalize a possibly-stringified number into a JSON number (or null).
fn number_or_null(v: Option<&Value>) -> Value {
    match v {
        Some(Value::Number(n)) => Value::Number(n.clone()),
        Some(Value::String(s)) => s
            .trim()
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        _ => Value::Null,
    }
}

/// gamma sends `outcomes`/`outcomePrices` as a JSON array encoded INSIDE a
/// string, e.g. `"[\"Yes\", \"No\"]"`. Decode it back to a real JSON value;
/// fall back to the raw value (or null) if it is not such an encoding.
fn decode_json_string(v: Option<&Value>) -> Value {
    match v {
        Some(Value::String(s)) => {
            serde_json::from_str::<Value>(s).unwrap_or_else(|_| Value::String(s.clone()))
        }
        Some(other) => other.clone(),
        None => Value::Null,
    }
}
