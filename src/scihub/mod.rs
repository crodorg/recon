//! Sci-Hub full-text retrieval.
//!
//! Two pieces: a self-refreshing **live-domain mirror** ([`mirror`]) and a
//! **fetch-by-identifier** path ([`fetch`]) that turns a DOI/PMID into a PDF on
//! disk — or an honest "not in the corpus".
//!
//! This is a **reading aid, deliberately walled off from the citation flow.**
//! Sci-Hub is a piracy mirror; the report still cites the DOI/publisher, never a
//! mirror URL (the credibility/citation verifiers already flag mirror hosts). So
//! nothing here registers a source, emits a citation, or touches a run's
//! sources.jsonl — it just hands you the bytes to read.
//!
//! Reality check baked into the UX: Sci-Hub's corpus has been **frozen since
//! 2021** (pending litigation), so anything published 2022+ is simply not
//! indexed. A miss on a recent paper is expected, not a bug — the miss message
//! says so.

pub mod fetch;
pub mod mirror;
