//! Deterministic claim-support scoring.
//!
//! Port of `verify_claim_support.py::compute_support_score`. Compares a claim
//! against the best-matching evidence quote across four dimensions (lexical
//! tokens, numbers, years, capitalized entities) and folds them into a single
//! weighted score that drives a coarse support status.

use std::collections::HashSet;
use std::sync::OnceLock;

use regex::Regex;

use crate::model::SupportResult;

// Capitalized entity strings that, on their own, carry no signal.
const STOP_ENTITIES: &[&str] = &[
    "The",
    "This",
    "That",
    "These",
    "However",
    "Furthermore",
    "Moreover",
    "Additionally",
    "Therefore",
    "Nevertheless",
];

// `\b\d+(?:\.\d+)?(?:%|x|X)?\b` — integers/decimals with an optional unit
// suffix. The trailing `\b` means a `%` is never actually captured (it is not a
// word char so no boundary follows it), while `x`/`X` are.
fn number_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\b\d+(?:\.\d+)?(?:%|x|X)?\b").unwrap())
}

// `\b(19|20)\d{2}\b` — year-like numbers. Mirrors Python `findall`, which with a
// single capturing group yields ONLY that group: i.e. "19" or "20", not the
// full four digits.
fn year_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\b(19|20)\d{2}\b").unwrap())
}

// `\b[A-Z][a-z]+(?:\s+[A-Z][a-z]+)*\b` — naive capitalized-entity NER.
fn entity_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\b[A-Z][a-z]+(?:\s+[A-Z][a-z]+)*\b").unwrap())
}

// `\b[a-z]{4,}\b` over the lowercased text.
fn token_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\b[a-z]{4,}\b").unwrap())
}

/// Significant lowercase tokens (>=4 chars), deduplicated.
fn extract_tokens(text: &str) -> HashSet<String> {
    let lower = text.to_lowercase();
    token_re()
        .find_iter(&lower)
        .map(|m| m.as_str().to_string())
        .collect()
}

/// Numeric values (full matches of `number_re`), deduplicated.
fn extract_numbers(text: &str) -> HashSet<String> {
    number_re()
        .find_iter(text)
        .map(|m| m.as_str().to_string())
        .collect()
}

/// Year mentions — the captured "19"/"20" prefix only, matching Python's
/// single-group `findall` behavior.
fn extract_years(text: &str) -> HashSet<String> {
    year_re()
        .captures_iter(text)
        .filter_map(|c| c.get(1).map(|m| m.as_str().to_string()))
        .collect()
}

/// Capitalized entity mentions minus the stop set.
fn extract_entities(text: &str) -> HashSet<String> {
    let stops: HashSet<&str> = STOP_ENTITIES.iter().copied().collect();
    entity_re()
        .find_iter(text)
        .map(|m| m.as_str().to_string())
        .filter(|e| !stops.contains(e.as_str()))
        .collect()
}

/// `|a & b| / |a|`, with an empty `a` yielding 0.0.
fn ratio(a: &HashSet<String>, b: &HashSet<String>) -> f64 {
    if a.is_empty() {
        return 0.0;
    }
    let inter = a.iter().filter(|x| b.contains(*x)).count();
    inter as f64 / a.len() as f64
}

/// Round to 3 decimals, matching Python's `round(x, 3)`.
///
/// Python rounds to the nearest decimal with ties-to-even, computed on the true
/// binary value of the double. Rust's `{:.3}` formatter does exactly the same
/// rounding, so formatting and reparsing reproduces `round(x, 3)` bit-for-bit
/// across every score this function can produce (verified by exhaustive diff
/// against the Python reference).
fn round3(x: f64) -> f64 {
    format!("{:.3}", x).parse::<f64>().unwrap()
}

/// Score a claim against its linked evidence quotes; returns the support status,
/// best composite score, and notes on weak dimensions.
pub fn compute(claim_text: &str, evidence_quotes: &[String]) -> SupportResult {
    if evidence_quotes.is_empty() {
        return SupportResult {
            status: "unsupported".to_string(),
            score: 0.0,
            notes: "no evidence linked".to_string(),
        };
    }

    let claim_tokens = extract_tokens(claim_text);
    let claim_numbers = extract_numbers(claim_text);
    let claim_years = extract_years(claim_text);
    let claim_entities = extract_entities(claim_text);

    let mut best_score = 0.0_f64;
    let mut best_notes: Vec<&str> = Vec::new();

    for quote in evidence_quotes {
        let ev_tokens = extract_tokens(quote);
        let ev_numbers = extract_numbers(quote);
        let ev_years = extract_years(quote);
        let ev_entities = extract_entities(quote);

        let token_overlap = ratio(&claim_tokens, &ev_tokens);
        // A claim with no items in a category passes that category by default.
        let number_match = if claim_numbers.is_empty() {
            1.0
        } else {
            ratio(&claim_numbers, &ev_numbers)
        };
        let year_match = if claim_years.is_empty() {
            1.0
        } else {
            ratio(&claim_years, &ev_years)
        };
        let entity_match = if claim_entities.is_empty() {
            1.0
        } else {
            ratio(&claim_entities, &ev_entities)
        };

        let score =
            0.4 * token_overlap + 0.25 * number_match + 0.15 * year_match + 0.2 * entity_match;

        if score > best_score {
            best_score = score;
            best_notes = Vec::new();
            if token_overlap < 0.3 {
                best_notes.push("low lexical overlap");
            }
            if !claim_numbers.is_empty() && number_match < 0.5 {
                best_notes.push("number mismatch");
            }
            if !claim_years.is_empty() && year_match < 1.0 {
                best_notes.push("year mismatch");
            }
            if !claim_entities.is_empty() && entity_match < 0.3 {
                best_notes.push("entity mismatch");
            }
        }
    }

    let status = if best_score >= 0.6 {
        "supported"
    } else if best_score >= 0.35 {
        "partial"
    } else {
        "needs_review"
    };

    let notes = if best_notes.is_empty() {
        "adequate overlap".to_string()
    } else {
        best_notes.join("; ")
    };

    SupportResult {
        status: status.to_string(),
        score: round3(best_score),
        notes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_quotes_unsupported() {
        let r = compute("anything", &[]);
        assert_eq!(r.status, "unsupported");
        assert_eq!(r.score, 0.0);
        assert_eq!(r.notes, "no evidence linked");
    }

    #[test]
    fn years_collapse_to_prefix() {
        // Python YEAR_RE captures only "20" group.
        let ys = extract_years("the 2020 study and 1999 paper");
        assert!(ys.contains("20"));
        assert!(ys.contains("19"));
        assert!(!ys.contains("2020"));
    }

    #[test]
    fn number_suffix_semantics() {
        let ns = extract_numbers("100% growth, 3.5x faster, 10X, 1000 items");
        // "%" is dropped by the trailing \b; x/X are kept.
        assert!(ns.contains("100"));
        assert!(ns.contains("3.5x"));
        assert!(ns.contains("10X"));
        assert!(ns.contains("1000"));
    }

    #[test]
    fn entity_stopword_filtering() {
        // The regex greedily joins adjacent Capitalized words, so "However Google"
        // is a single entity (not the bare stopword "However") and survives;
        // a standalone "The" / "However" would be dropped. Matches the Python ref.
        let es = extract_entities("The United States and However Google");
        assert!(es.contains("The United States"));
        assert!(es.contains("However Google"));
        assert_eq!(es.len(), 2);
        let bare = extract_entities("The However stand alone");
        assert!(!bare.contains("The"));
        assert!(!bare.contains("However"));
    }

    #[test]
    fn strong_support() {
        let claim = "Transformers improved accuracy by 5% in 2020 at Google";
        let quotes = vec![
            "At Google the Transformers architecture improved accuracy by 5% in 2020".to_string(),
        ];
        let r = compute(claim, &quotes);
        assert_eq!(r.status, "supported");
        assert!(r.score >= 0.6);
    }

    #[test]
    fn entity_mismatch_lands_partial() {
        // Claim has an entity ("Quantum") and 4+ char tokens but the quote shares
        // neither. token_overlap=0, entity_match=0, but numbers/years default to
        // 1.0, so score = 0.25 + 0.15 = 0.4 -> partial. Matches the Python ref.
        let claim = "Quantum entanglement enables teleportation experiments";
        let quotes = vec!["Bananas grow in tropical climates worldwide".to_string()];
        let r = compute(claim, &quotes);
        assert_eq!(r.status, "partial");
        assert_eq!(r.score, 0.4);
        assert_eq!(r.notes, "low lexical overlap; entity mismatch");
    }

    #[test]
    fn token_number_entity_all_miss_needs_review() {
        // Claim has tokens, a number, and an entity; the quote shares none of
        // them. Only the (absent) year category defaults to 1.0, so
        // score = 0.15 -> needs_review. Verified against the Python reference.
        let claim = "Quantum systems achieve 99 results";
        let quotes = vec!["bananas grow well".to_string()];
        let r = compute(claim, &quotes);
        assert_eq!(r.status, "needs_review");
        assert_eq!(r.score, 0.15);
        assert_eq!(
            r.notes,
            "low lexical overlap; number mismatch; entity mismatch"
        );
    }
}
