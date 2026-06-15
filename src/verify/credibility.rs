//! Deterministic source-credibility scoring (no network, no LLM).
//!
//! Ported from `bio/scripts/source_evaluator.py`. The Python `evaluate_source`
//! takes a `content` argument used only for a balanced-language neutrality
//! bonus; the frozen `score()` signature has no `content`, so that one branch is
//! intentionally dropped. Everything else is a faithful port.
//!
//! `overall = 0.35*domain + 0.25*expertise + 0.20*recency + 0.20*neutrality`,
//! every component on a 0-100 scale. Python's `bias_score` is this crate's
//! `neutrality` (higher = more neutral).
//!
//! Domain authority has two layers: a NEUTRAL built-in tier list (so the public
//! crate ships no one's worldview), and an optional user [`TrustConfig`] loaded
//! from `~/.config/recon/trust.conf` whose `trusted`/`independent`/`distrusted`
//! tiers override the defaults. Heterodox sources belong in the user's config, not
//! compiled in. `score()` keeps its frozen signature (empty config); `score_with`
//! takes the config.

use std::path::PathBuf;

use chrono::{DateTime, NaiveDate, Utc};
use serde_json::{Map, Value};

use crate::model::Credibility;

// ---------------------------------------------------------------------------
// Domain reputation tiers (ported + extended)
// ---------------------------------------------------------------------------

/// High-authority domains. The first block mirrors the Python HIGH list; the
/// second block is our primary-source extension (legal/regulatory/medical/econ).
const HIGH_AUTHORITY_DOMAINS: &[&str] = &[
    // Academic & Research
    "doi.org", // registered scholarly work — in this pipeline a doi.org URL only
    // ever comes from the OpenAlex/Crossref connectors (Phase 9), so it denotes a
    // real academic record; consistent with arxiv.org (a preprint server) being HIGH.
    "arxiv.org",
    "nature.com",
    "science.org",
    "cell.com",
    "nejm.org",
    "thelancet.com",
    "springer.com",
    "sciencedirect.com",
    "plos.org",
    "ieee.org",
    "acm.org",
    "pubmed.ncbi.nlm.nih.gov",
    // Government & International Organizations
    "nih.gov",
    "cdc.gov",
    "who.int",
    "fda.gov",
    "nasa.gov",
    "gov.uk",
    "europa.eu",
    "un.org",
    // Established Tech Documentation
    "docs.python.org",
    "developer.mozilla.org",
    "docs.microsoft.com",
    "cloud.google.com",
    "aws.amazon.com",
    "kubernetes.io",
    // --- Our primary-source extension ---
    "uscode.house.gov",
    "ecfr.gov",
    "govinfo.gov",
    "courtlistener.com",
    "regulations.gov",
    "sec.gov",
    "federalregister.gov",
    "cochranelibrary.com",
    "clinicaltrials.gov",
    "bls.gov",
    "census.gov",
    "federalreserve.gov",
    // Primary economic/financial data (de-bias add 2026-06-09).
    "fred.stlouisfed.org",
    "pages.stern.nyu.edu", // Damodaran datasets (NYU Stern)
];

// NOTE (de-bias, 2026-06-09): establishment news is NOT primary evidence. Wire
// services + quality papers are tier-5 journalism with their own institutional
// bias on contested topics, so they sit at MODERATE, not HIGH — equating them
// with peer review (HIGH) fought the engine's own contrarian axis.

const MODERATE_AUTHORITY_DOMAINS: &[&str] = &[
    // Reputable news / wire services (tier-5 journalism, not primary — moved
    // down from HIGH 2026-06-09).
    "reuters.com",
    "apnews.com",
    "bbc.com",
    "economist.com",
    "scientificamerican.com",
    // Tech News & Analysis
    "techcrunch.com",
    "theverge.com",
    "arstechnica.com",
    "wired.com",
    "zdnet.com",
    "cnet.com",
    // Industry Publications
    "forbes.com",
    "bloomberg.com",
    "wsj.com",
    "ft.com",
    // Educational
    "wikipedia.org",
    "britannica.com",
    "khanacademy.org",
    // Tech Blogs (established)
    "medium.com",
    "dev.to",
    "stackoverflow.com",
    "github.com",
];

// Low-quality PLATFORM signals. Note (2026-06-09): substack.com + wordpress.com
// were removed — they host serious independent journalists and working academics,
// so penalizing the platform (not the source) suppressed exactly the heterodox
// voices the contrarian axis is meant to surface. blogspot/wix remain weak signals.
const LOW_AUTHORITY_INDICATORS: &[&str] = &["blogspot.com", "wix.com"];

// ---------------------------------------------------------------------------
// User trust config (external, neutral-crate-friendly override)
// ---------------------------------------------------------------------------

/// User-curated domain trust tiers, loaded from `~/.config/recon/trust.conf`.
/// Overrides the built-in tiers so the published crate stays neutral and the
/// operator tunes their own trust map (and flags conspiracy/pseudoscience as
/// `distrusted`). Empty = no overrides (the built-in defaults stand).
#[derive(Clone, Debug, Default)]
pub struct TrustConfig {
    /// Treat as high authority (domain score 90).
    pub trusted: Vec<String>,
    /// Independent / heterodox-but-legitimate: don't penalize the platform, judge
    /// on merits (domain score 60 — include, not authority).
    pub independent: Vec<String>,
    /// User-flagged unreliable (conspiracy/pseudoscience): domain score 20 —
    /// surfaced but bottom-ranked and flagged; never an authority.
    pub distrusted: Vec<String>,
}

impl TrustConfig {
    /// Default config path: `$XDG_CONFIG_HOME/recon/trust.conf` (falls back to
    /// `$HOME/.config/recon/trust.conf`).
    pub fn default_path() -> Option<PathBuf> {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
        Some(base.join("recon").join("trust.conf"))
    }

    /// Load from the default path. Missing file or any read/parse error yields an
    /// empty config — trust curation must never fail a run.
    pub fn load() -> Self {
        match Self::default_path() {
            Some(p) => std::fs::read_to_string(&p)
                .map(|s| Self::parse(&s))
                .unwrap_or_default(),
            None => Self::default(),
        }
    }

    /// Parse the dead-simple section format (no deps): `[trusted]`/`[independent]`/
    /// `[distrusted]` headers; one bare domain per line; `#` comments (full-line or
    /// trailing/inline) + blanks ignored; unknown sections skipped.
    pub fn parse(text: &str) -> Self {
        let mut cfg = Self::default();
        let mut section: Option<&mut Vec<String>> = None;
        for line in text.lines() {
            // Strip any inline comment (domains never contain '#'), then trim.
            let l = line.split('#').next().unwrap_or("").trim();
            if l.is_empty() {
                continue;
            }
            if let Some(name) = l.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
                section = match name.trim().to_ascii_lowercase().as_str() {
                    "trusted" => Some(&mut cfg.trusted),
                    "independent" => Some(&mut cfg.independent),
                    "distrusted" => Some(&mut cfg.distrusted),
                    _ => None,
                };
                continue;
            }
            if let Some(list) = section.as_deref_mut() {
                list.push(l.to_ascii_lowercase());
            }
        }
        cfg
    }
}

/// True if `domain` equals `entry` or is a subdomain of it (so a config entry of
/// `nyu.edu` matches `pages.stern.nyu.edu`).
fn domain_in(list: &[String], domain: &str) -> bool {
    list.iter()
        .any(|e| domain == e || domain.ends_with(&format!(".{e}")))
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Score a source's credibility from URL/title/date/author, with no user trust
/// overrides. Pure, deterministic, no I/O. Frozen signature (Python-port parity),
/// exercised by the test suite; delegates to [`score_with`] with an empty
/// [`TrustConfig`]. The binary's commands call `score_with` directly.
#[allow(dead_code)]
pub fn score(url: &str, title: &str, date: Option<&str>, author: Option<&str>) -> Credibility {
    score_with(url, title, date, author, &TrustConfig::default())
}

/// Score a source's credibility, consulting the user [`TrustConfig`] for domain
/// authority (it overrides the built-in tiers). Deterministic, no I/O.
pub fn score_with(
    url: &str,
    title: &str,
    date: Option<&str>,
    author: Option<&str>,
    trust: &TrustConfig,
) -> Credibility {
    let domain = extract_domain(url);

    let domain_score = evaluate_domain_authority(&domain, trust);
    let recency_score = evaluate_recency(date);
    let expertise_score = evaluate_expertise(&domain, title, author);
    let neutrality_score = evaluate_neutrality(&domain, title);

    // Weighted overall (mirrors the Python weights exactly).
    let overall = domain_score * 0.35
        + expertise_score * 0.25
        + recency_score * 0.20
        + neutrality_score * 0.20;

    let factors = identify_factors(
        domain_score,
        recency_score,
        expertise_score,
        neutrality_score,
    );
    let recommendation = generate_recommendation(overall);

    Credibility {
        overall: round2(overall),
        domain: round2(domain_score),
        recency: round2(recency_score),
        expertise: round2(expertise_score),
        neutrality: round2(neutrality_score),
        recommendation,
        factors,
    }
}

// ---------------------------------------------------------------------------
// Component scorers
// ---------------------------------------------------------------------------

/// Extract a lowercased host with a leading `www.` stripped. Falls back to a
/// best-effort parse when the input isn't a well-formed URL (mirrors Python's
/// lenient `urlparse().netloc`).
fn extract_domain(url: &str) -> String {
    if let Ok(parsed) = url::Url::parse(url.trim()) {
        if let Some(host) = parsed.host_str() {
            let host = host.to_ascii_lowercase();
            return host.strip_prefix("www.").unwrap_or(&host).to_string();
        }
    }
    // Fallback: strip scheme, take the authority up to the first `/?#`.
    let s = url.trim();
    let s = s.split("://").last().unwrap_or(s);
    let authority = s
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    // Drop any userinfo and port.
    let authority = authority.rsplit('@').next().unwrap_or(&authority);
    let host = authority.split(':').next().unwrap_or(authority);
    host.strip_prefix("www.").unwrap_or(host).to_string()
}

/// Domain authority on 0-100. User trust config wins (distrusted 20 / trusted 90 /
/// independent 60), then built-in tiers: HIGH~90, MODERATE~70, low-authority
/// platform indicators~40, unknown~55.
fn evaluate_domain_authority(domain: &str, trust: &TrustConfig) -> f64 {
    // User overrides first, most-skeptical wins on overlap.
    if domain_in(&trust.distrusted, domain) {
        20.0
    } else if domain_in(&trust.trusted, domain) {
        90.0
    } else if domain_in(&trust.independent, domain) {
        60.0
    } else if HIGH_AUTHORITY_DOMAINS.contains(&domain) {
        90.0
    } else if MODERATE_AUTHORITY_DOMAINS.contains(&domain) {
        70.0
    } else if LOW_AUTHORITY_INDICATORS
        .iter()
        .any(|ind| domain.contains(ind))
    {
        40.0
    } else {
        // Unknown domain - moderate skepticism.
        55.0
    }
}

/// Recency on 0-100 from a publication date. <90d=100, <1y=85, <2y=70, <5y=50,
/// else 30. Missing or unparseable date => 50.
fn evaluate_recency(date: Option<&str>) -> f64 {
    let raw = match date {
        Some(d) if !d.trim().is_empty() => d.trim(),
        _ => return 50.0,
    };

    let pub_date = match parse_date(raw) {
        Some(d) => d,
        None => return 50.0,
    };

    let now = Utc::now().date_naive();
    // Age in whole days; future dates clamp to 0 (treated as freshest).
    let age_days = (now - pub_date).num_days().max(0);

    if age_days < 90 {
        100.0
    } else if age_days < 365 {
        85.0
    } else if age_days < 730 {
        70.0
    } else if age_days < 1825 {
        50.0
    } else {
        30.0
    }
}

/// Expertise on 0-100. Base 50, plus bonuses for academic / government /
/// documentation signals and author credentials. Clamped to 100.
fn evaluate_expertise(domain: &str, title: &str, author: Option<&str>) -> f64 {
    let mut score = 50.0_f64;

    // Academic/recon domains.
    let academic = ["arxiv", "nature", "science", "ieee", "acm"];
    if academic.iter().any(|d| domain.contains(d)) {
        score += 30.0;
    }

    // Government / official sources.
    if domain.contains(".gov") || domain.contains("who.int") {
        score += 25.0;
    }

    // Technical documentation.
    if domain.contains("docs.") || title.to_lowercase().contains("documentation") {
        score += 20.0;
    }

    // Author credentials.
    if let Some(a) = author {
        let a = a.to_lowercase();
        if ["dr.", "phd", "professor"].iter().any(|t| a.contains(t)) {
            score += 15.0;
        }
    }

    score.min(100.0)
}

/// Neutrality on 0-100 (higher = more neutral). Base 70, minus sensationalism in
/// the title, plus an academic-domain bonus. Clamped to 0-100.
///
/// NOTE: the Python original also adds a +10 bonus when `content` contains
/// balanced-language markers. The frozen `score()` signature carries no
/// `content`, so that branch is omitted by design.
fn evaluate_neutrality(domain: &str, title: &str) -> f64 {
    let mut score = 70.0_f64;

    let title_lower = title.to_lowercase();
    let sensational = [
        "!",
        "shocking",
        "unbelievable",
        "you won't believe",
        "secret",
        "they don't want you to know",
    ];
    if sensational.iter().any(|s| title_lower.contains(s)) {
        score -= 20.0;
    }

    // Academic sources are typically less biased.
    let academic = ["arxiv", "nature", "science", "ieee"];
    if academic.iter().any(|d| domain.contains(d)) {
        score += 20.0;
    }

    score.clamp(0.0, 100.0)
}

// ---------------------------------------------------------------------------
// Factors + recommendation
// ---------------------------------------------------------------------------

/// Build the notable-factors JSON object (only thresholds that fire are
/// included), mirroring the Python `_identify_factors`.
fn identify_factors(
    domain_score: f64,
    recency_score: f64,
    expertise_score: f64,
    neutrality_score: f64,
) -> Value {
    let mut map = Map::new();

    if domain_score >= 85.0 {
        map.insert("domain".into(), Value::from("High authority domain"));
    } else if domain_score <= 45.0 {
        map.insert(
            "domain".into(),
            Value::from("Low authority domain - verify claims"),
        );
    }

    if recency_score >= 85.0 {
        map.insert("recency".into(), Value::from("Recent information"));
    } else if recency_score <= 40.0 {
        map.insert(
            "recency".into(),
            Value::from("Outdated information - verify currency"),
        );
    }

    if expertise_score >= 80.0 {
        map.insert("expertise".into(), Value::from("Expert source"));
    } else if expertise_score <= 45.0 {
        map.insert(
            "expertise".into(),
            Value::from("Limited expertise indicators"),
        );
    }

    // Python keys this "bias"; this crate's component is `neutrality`, but the
    // human-facing factor strings are about bias. Keep the Python key.
    if neutrality_score >= 80.0 {
        map.insert("bias".into(), Value::from("Balanced perspective"));
    } else if neutrality_score <= 50.0 {
        map.insert("bias".into(), Value::from("Potential bias detected"));
    }

    Value::Object(map)
}

/// Trust recommendation from the overall score.
fn generate_recommendation(overall: f64) -> String {
    if overall >= 80.0 {
        "high_trust"
    } else if overall >= 60.0 {
        "moderate_trust"
    } else if overall >= 40.0 {
        "low_trust"
    } else {
        "verify"
    }
    .to_string()
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Round to 2 decimals (matches Python `round(x, 2)` closely enough for display;
/// uses round-half-away-from-zero, which is fine for non-negative scores).
fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// Parse a publication date as either a full ISO-8601/RFC3339 timestamp or a
/// bare `YYYY-MM-DD` date. Mirrors Python's `datetime.fromisoformat` leniency
/// over the inputs this engine actually emits, including a trailing `Z`.
fn parse_date(raw: &str) -> Option<NaiveDate> {
    let s = raw.trim();

    // Full RFC3339 timestamp (handles the trailing `Z` and offsets).
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&Utc).date_naive());
    }
    // Python normalizes `...Z` to `+00:00`; try that too for non-strict forms.
    if let Some(stripped) = s.strip_suffix('Z') {
        let swapped = format!("{}+00:00", stripped);
        if let Ok(dt) = DateTime::parse_from_rfc3339(&swapped) {
            return Some(dt.with_timezone(&Utc).date_naive());
        }
    }

    // Bare date `YYYY-MM-DD`.
    if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Some(d);
    }

    // Datetime with a space separator and no zone: `YYYY-MM-DD HH:MM:SS`.
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S") {
        return Some(dt.date());
    }
    // ...with a `T` separator and no zone.
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S") {
        return Some(dt.date());
    }

    // Year-month only: treat as the first of the month.
    if let Ok(d) = NaiveDate::parse_from_str(&format!("{s}-01"), "%Y-%m-%d") {
        return Some(d);
    }

    // Bare year: Jan 1 of that year.
    if s.len() == 4 && s.chars().all(|c| c.is_ascii_digit()) {
        if let Ok(year) = s.parse::<i32>() {
            return NaiveDate::from_ymd_opt(year, 1, 1);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn high_authority_recent_academic() {
        let c = score(
            "https://www.nature.com/articles/s41586-2025-12345",
            "Breakthrough in Quantum Computing",
            Some("2099-01-01"), // future -> treated as freshest
            None,
        );
        assert_eq!(c.domain, 90.0);
        assert_eq!(c.recency, 100.0);
        // expertise: base 50 + academic 30 = 80
        assert_eq!(c.expertise, 80.0);
        // neutrality: 70 + academic 20 = 90
        assert_eq!(c.neutrality, 90.0);
        assert_eq!(c.recommendation, "high_trust");
        assert_eq!(c.factors["bias"], "Balanced perspective");
    }

    #[test]
    fn doi_url_is_high_authority() {
        // Scholarly hits from the OpenAlex/Crossref connectors carry doi.org URLs;
        // doi.org must score HIGH so a canonical paper isn't triaged out as low-trust.
        let c = score(
            "https://doi.org/10.1038/171737a0",
            "Molecular Structure of Nucleic Acids",
            Some("1953-04-25"), // old -> recency 30
            None,
        );
        assert_eq!(c.domain, 90.0);
        // overall = 90*.35 + 50*.25 + 30*.20 + 70*.20 = 31.5+12.5+6+14 = 64.0
        assert_eq!(c.overall, 64.0);
        assert_eq!(c.recommendation, "moderate_trust");
    }

    #[test]
    fn low_authority_sensational_old() {
        // blogspot.com is still a low-quality-platform signal (wordpress/substack
        // were removed from the indicators 2026-06-09).
        let c = score(
            "https://someblog.blogspot.com/shocking-discovery",
            "SHOCKING! You Won't Believe This Discovery!",
            Some("2010-01-01"),
            None,
        );
        assert_eq!(c.domain, 40.0);
        assert_eq!(c.recency, 30.0);
        // neutrality: 70 - 20 (sensational) = 50
        assert_eq!(c.neutrality, 50.0);
        // overall = 40*.35 + 50*.25 + 30*.20 + 50*.20 = 14+12.5+6+10 = 42.5
        assert_eq!(c.overall, 42.5);
        // 42.5 is in [40,60) -> low_trust (matches the Python reference)
        assert_eq!(c.recommendation, "low_trust");
        assert_eq!(c.factors["domain"], "Low authority domain - verify claims");
    }

    #[test]
    fn primary_source_extension_is_high() {
        for d in [
            "https://www.courtlistener.com/opinion/123/foo/",
            "https://uscode.house.gov/view.xhtml",
            "https://www.sec.gov/cgi-bin/browse-edgar",
            "https://clinicaltrials.gov/study/NCT00000000",
        ] {
            let c = score(d, "Some Filing", None, None);
            assert_eq!(c.domain, 90.0, "expected HIGH for {d}");
        }
    }

    #[test]
    fn government_and_docs_expertise() {
        // .gov bonus
        let c = score(
            "https://www.bls.gov/cpi/",
            "Consumer Price Index",
            None,
            None,
        );
        // base 50 + .gov 25 = 75
        assert_eq!(c.expertise, 75.0);

        // docs. bonus
        let c2 = score(
            "https://docs.python.org/3/library/asyncio.html",
            "asyncio — Asynchronous I/O",
            None,
            None,
        );
        // base 50 + docs 20 = 70
        assert_eq!(c2.expertise, 70.0);
    }

    #[test]
    fn author_credential_bonus() {
        let c = score(
            "https://example.com/post",
            "Title documentation",
            None,
            Some("Dr. Jane Smith, PhD"),
        );
        // base 50 + docs(title 'documentation') 20 + author 15 = 85
        assert_eq!(c.expertise, 85.0);
    }

    #[test]
    fn unknown_domain_none_date() {
        let c = score("https://example.com/article", "A Plain Title", None, None);
        assert_eq!(c.domain, 55.0);
        assert_eq!(c.recency, 50.0);
        assert_eq!(c.expertise, 50.0);
        assert_eq!(c.neutrality, 70.0);
        // overall = 55*.35 + 50*.25 + 50*.20 + 70*.20 = 19.25+12.5+10+14 = 55.75
        assert_eq!(c.overall, 55.75);
        assert_eq!(c.recommendation, "low_trust");
    }

    #[test]
    fn parses_rfc3339_and_z() {
        assert!(parse_date("2025-10-15T08:30:00Z").is_some());
        assert!(parse_date("2025-10-15T08:30:00+00:00").is_some());
        assert!(parse_date("2025-10-15").is_some());
        assert!(parse_date("2025-10").is_some());
        assert!(parse_date("2025").is_some());
        assert!(parse_date("not a date").is_none());
    }

    #[test]
    fn factors_is_json_object() {
        let c = score("https://example.com", "Plain", None, None);
        assert!(c.factors.is_object());
    }

    // ---- de-bias (2026-06-09) ----

    #[test]
    fn establishment_news_is_moderate_not_high() {
        for d in [
            "https://www.reuters.com/x",
            "https://apnews.com/y",
            "https://www.bbc.com/news/z",
        ] {
            assert_eq!(
                score(d, "t", None, None).domain,
                70.0,
                "expected MODERATE for {d}"
            );
        }
    }

    #[test]
    fn self_publishing_platforms_not_penalized() {
        // substack/wordpress no longer auto-dinged → fall through to unknown 55.
        assert_eq!(
            score("https://writer.substack.com/p/x", "t", None, None).domain,
            55.0
        );
        assert_eq!(
            score("https://someone.wordpress.com/x", "t", None, None).domain,
            55.0
        );
        // blogspot/wix still flagged low.
        assert_eq!(
            score("https://x.blogspot.com/y", "t", None, None).domain,
            40.0
        );
    }

    #[test]
    fn primary_financial_data_is_high() {
        assert_eq!(
            score(
                "https://fred.stlouisfed.org/series/CPIAUCSL",
                "CPI",
                None,
                None
            )
            .domain,
            90.0
        );
        assert_eq!(
            score(
                "https://pages.stern.nyu.edu/~adamodar/data.html",
                "Damodaran",
                None,
                None
            )
            .domain,
            90.0
        );
    }

    // ---- trust config overrides ----

    #[test]
    fn trust_config_overrides_built_in_tiers() {
        let cfg = TrustConfig {
            trusted: vec!["myprimary.example".into()],
            independent: vec!["lukesmith.xyz".into()],
            distrusted: vec!["whale.to".into()],
        };
        // distrusted bottoms out
        assert_eq!(
            score_with("https://whale.to/a", "t", None, None, &cfg).domain,
            20.0
        );
        // trusted lifts an otherwise-unknown domain to HIGH
        assert_eq!(
            score_with("https://myprimary.example/x", "t", None, None, &cfg).domain,
            90.0
        );
        // independent: not penalized, not authority
        assert_eq!(
            score_with("https://lukesmith.xyz/post", "t", None, None, &cfg).domain,
            60.0
        );
    }

    #[test]
    fn distrusted_wins_over_other_tiers_and_matches_subdomains() {
        let cfg = TrustConfig {
            // bbc.com is built-in MODERATE; distrust must override it.
            distrusted: vec!["bbc.com".into(), "nyu.edu".into()],
            ..Default::default()
        };
        assert_eq!(
            score_with("https://www.bbc.com/news", "t", None, None, &cfg).domain,
            20.0
        );
        // subdomain match: nyu.edu entry catches pages.stern.nyu.edu
        assert_eq!(
            score_with("https://pages.stern.nyu.edu/x", "t", None, None, &cfg).domain,
            20.0
        );
    }

    #[test]
    fn trust_config_parse_sections_and_comments() {
        // Includes INLINE comments + aligned whitespace (the real config's shape).
        let cfg = TrustConfig::parse(
            "# my trust map\n[trusted]\nFred.StLouisFed.org   # primary data\n\n[distrusted]\nwhale.to    # anti-vax\n# full-line note\nmontalk.net\n[bogus]\nignored.com\n[independent]\nlukesmith.xyz # independent\n",
        );
        assert_eq!(cfg.trusted, vec!["fred.stlouisfed.org"]); // lowercased, comment stripped
        assert_eq!(cfg.distrusted, vec!["whale.to", "montalk.net"]);
        assert_eq!(cfg.independent, vec!["lukesmith.xyz"]);
    }
}
