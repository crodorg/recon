# recon — source routing & hierarchies

The `recon` binary owns retrieval; **this file is the map the skill uses to decide
*which* connectors to fire and *how* to judge what comes back.** Routing is judgment, so
it lives here (Claude reads it), not hard-coded in the binary. The binary just takes
`--sources a,b,c`.

Core rule: **don't fan out to everything by reflex.** Match the connectors to the
question. Asking a social feed about a federal statute is noise; asking the U.S. Code
whether a laptop is worth buying is useless. Pick the modality the answer actually lives in.

---

## Routing profiles

Each profile is a `--sources` list. **Perplexity is in every profile** — it owns web
breadth. The real decision is what to *add* (community, social, markets) and, for
authoritative work, what to *suppress*.

| Profile | `--sources` | Use for | Social? |
|---|---|---|---|
| **authoritative** | `perplexity` | law, regulation, compliance, statutes, courts, medical/health, drugs, finance/tax/securities, safety, government | **No** — social is noise *and* a liability; trace to primary text |
| **technical** | `perplexity,github,hn,grok` | coding, libraries, frameworks, devtools, infra, protocols, system design | Yes (practitioner) |
| **social** | `perplexity,grok,reddit` | relationships, parenting, dating, career advice, consumer-product opinions, hobbies, "is X worth it", "what do people think/feel" | **Yes** — lived experience is the point (X + Reddit) |
| **markets** | `perplexity,grok,polymarket` | predictions, elections, "will X happen", odds, event probabilities | Yes |
| **general** | `perplexity,hn,grok` | mixed / everything else | Light |

**Social = X + Reddit, both via `grok`.** Our *own* fetcher can't reach Reddit (every
User-Agent incl. a real browser, and `oauth.reddit.com` without a token, return Reddit's 403
block page; third-party scrapers are banned here). But **Grok can** — it reaches Reddit through
`old.reddit.com` + the PullPush archive API. So the `grok` connector (origin `grok-x`) covers
X/Twitter — news, breaking, broad social — and the `reddit` connector (origin `grok-reddit`,
also a Grok shell-out) covers human opinion, niche communities, and local/lived knowledge. Both
are tier 7-8 social: capped, traced to primary, never the sole basis for a load-bearing factual
claim. Route `reddit` in when the question wants *what people actually experienced* (esp. local
or niche), `grok`/X when it wants *what's being said now*.

### Classifying a query into a profile

Read the *intent*, not just keywords (a query about "Apple" could be the company, the
fruit, or a court case). Cues:

- **authoritative** — "is it legal", statute/regulation/section numbers, "FDA approved",
  "side effects of", diagnosis/treatment, "tax treatment of", SEC/10-K/earnings,
  jurisdiction words (federal, state, EU), "compliance", "court ruled". High cost of error.
- **technical** — language/library/framework/tool names, "how do I implement", "X vs Y"
  for software, error messages, architecture, "benchmark", repo/package names.
- **social** — "should I", "what's it like to", relationships/family/dating, "worth it",
  "people's experience with", subjective/consumer/lifestyle, sentiment.
- **markets** — "will ___ happen", "odds of", "probability", elections, "who will win".
- **general** — news, broad explainers, mixed-intent, or genuinely unsure.

When a query straddles (e.g. "is the new SEC crypto rule going to pass" = authoritative +
markets), union the connector lists but keep social *off* if any component is
authoritative. **Authoritative suppression wins:** when the answer must be right and
defensible, opinion sources don't earn a seat.

**Primary-tracing is a separate axis from social suppression.** A viability/"should I build
this" question can be `general` or `social` (social ON) yet still rest on legal/regulatory/
health/finance facts that must be traced to primary text. For those, set `trace_primary: true`
(SKILL.md Step 1) — it keeps social in the pool *and* makes the read + synthesis agents chase the
statute/filing/dataset instead of stopping at a blog. Don't conflate "keep social" with "skip the
primary": a regulatory core earns primary-tracing whether or not opinion sources also have a seat.

---

## Primary-source hierarchies (authoritative profile)

Secondary sources are **lead-gen only** — they tell you what to go read. Trace every
load-bearing claim to primary text. In deep mode the read agents fetch down this ladder;
in quick mode, at least name where the primary source *is*.

- **legal:** U.S. Code / eCFR / state codes → CourtListener + RECAP / regulations.gov /
  agency dockets / EDGAR → labeled agency guidance (non-binding) → commentary (caveated).
  Tag jurisdiction + as-of date; check subsequent history (superseded / overruled).
- **health:** PubMed / Cochrane / clinical practice guidelines / FDA labels → systematic
  reviews → narrative reviews → commentary. Prefer meta-analyses; flag retractions.
  - *Discovery:* for health/medical/scientific cores, add `openalex,crossref` to the source
    split — they surface the primary literature (DOIs, abstracts) that Perplexity's web breadth
    misses. OpenAlex = relevance discovery; Crossref = DOI/metadata authority + corroborator.
  - *Full text behind a paywall?* The deep read step **auto-fetches** it via Sci-Hub for any
    load-bearing paywalled paper (`recon fetch-paper <doi|pmid>`, corpus frozen at 2021) —
    a **reading aid only, cite the DOI, never the mirror**. Conservative: load-bearing papers
    only, no fan-out, 2022+ misses expected. Run it by hand for a one-off. See SKILL.md.
  - *Load-bearing primary you can't read in full?* **Surface the gap, don't bury it** — but aggregate.
    Covers a paper paywalled AND outside the corpus, OR any statute/filing/.gov/PDF/dataset that
    blocked every fetch rung (direct + Grok). The read step records a `provenance:"inaccessible"`
    marker; synthesis reports it by *finding*, not per source (one miss → a plain notice; many → the
    net "N primaries on X were inaccessible; the M read agree, so the verdict holds", or name them +
    downgrade if a claim rested only on the missed ones), and the workflow returns the count as
    `sources_inaccessible`. Never a wall of per-source "couldn't access" lines.
- **finance / markets:** EDGAR filings / official statistics / primary datasets / exchange
  data → analyst coverage → commentary. Prefer the filing over the article about it.

Primary-source domains worth biasing toward (the binary's credibility scorer already
ranks these high): `uscode.house.gov`, `ecfr.gov`, `courtlistener.com`, `regulations.gov`,
`sec.gov`, `pubmed.ncbi.nlm.nih.gov`, `cochranelibrary.com`, `fda.gov`, `*.gov`, `*.edu`.

---

## Source-quality ladder (for labeling, all profiles)

Sits above the domain hierarchies. Label every cited source by tier; **never cite tier
6–8 as the sole basis for a load-bearing factual claim** — corroborate or downgrade the
claim's confidence.

1. Peer-reviewed meta-analyses / systematic reviews
2. Peer-reviewed primary studies; official primary documents (statutes, filings, datasets)
3. Authoritative orgs / government agencies / standards bodies
4. Expert practitioners; official project documentation; reputable technical references
5. Quality journalism / established secondary sources
6. Industry blogs / trade press / vendor marketing
7. Community discussion (HN, GitHub issues, Q&A threads)
8. Social posts / forums / anonymous opinion

Confidence ladder for synthesized claims: **High** (multiple tier 1–3, no credible
conflict) / **Medium** (tier 3–5 or single strong source) / **Low** (tier 6–8 or thin) /
**Unverified** (asserted, not yet traced). State it.

---

## Trust config & heterodox sources

The binary scores domain authority from a **neutral built-in tier list** plus an optional
user override at `~/.config/recon/trust.conf` (`[trusted]` → 90, `[independent]` → 60,
`[distrusted]` → 20; subdomains match; most-skeptical tier wins). This is where the operator's
curation lives — the published crate ships no one's worldview.

Two corrections baked into the de-biased defaults, worth remembering when you label sources:

- **Establishment news is tier-5 journalism, not primary evidence.** Reuters/AP/BBC/Economist
  sit at MODERATE (70), not HIGH — they carry institutional bias on contested topics. Don't cite
  them as the authority for a load-bearing factual claim; trace to the primary they're reporting on.
- **Self-publishing platform ≠ low quality.** Substack/WordPress aren't auto-penalized — serious
  independent journalists and academics publish there. Judge the source, not the host.

**Heterodox sources (the `[distrusted]` tier and tier 7–8 generally):** the contrarian query axis
surfaces them on purpose, so synthesis *sees* the dissent — but they are **capped, never the sole
basis for a load-bearing factual claim**, and traced to primary exactly like social signal. A verdict
on a health/legal/finance question must not rest on a conspiracy or pseudoscience source no matter how
confidently it asserts. Include the dissent; don't let it drive.

## Connector reference

What the binary's `--sources` names do. All run concurrently; a failing connector logs to
stderr and is skipped — it never crashes the run.

| name | modality | cost | notes |
|---|---|---|---|
| `perplexity` | web breadth (ranked results) | $5 / 1k requests, no tokens | needs `PERPLEXITY_API_KEY`; returns title/url/snippet/last_updated |
| `grok` | X/Twitter + web (social) | $0 marginal (SuperGrok sub) | shells out to the `grok` CLI; ~10–120s; origin `grok-x` |
| `reddit` | Reddit (human opinion / niche / local) | $0 marginal (SuperGrok sub) | shells to the `grok` CLI via `old.reddit.com` + PullPush; ~10–120s; origin `grok-reddit`; tier 7-8 |
| `github` | repos | free (higher limit with `GITHUB_TOKEN`) | **repo-name/description search — feed it keywords, not a sentence** |
| `hn` | Hacker News (Algolia) | free | tech/startup discussion; carries points + comments |
| `polymarket` | prediction markets | free | gamma API, no full-text search → client-side filter; best-effort |
| `openalex` | open scholarly works (discovery) | free at our volume | optional `OPENALEX_API_KEY` + `OPENALEX_MAILTO` (env); works keyless for light use. Returns DOI/title/abstract/year; origin `openalex`, `source_type:academic` |
| `crossref` | DOI/metadata authority (scholarly) | free, no key | optional `CROSSREF_MAILTO` (env) for the polite pool. DOI registry of record; origin `crossref`, `source_type:academic` |

Retrieval is network-bound; the cost knob is how many queries the local loop issues, not
the connectors themselves.
