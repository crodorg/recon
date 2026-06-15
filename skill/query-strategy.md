# research — query strategy (the decomposition doctrine)

The dominant failure mode of deep research is **"the critical source was never surfaced,"** not
"we under-read a source." So the whole game is decomposition: turn one question into many good
queries that hit the answer from every angle, fire them cheap and wide (Perplexity is
$0.005/query), and read only the best survivors. This file is the doctrine the **strategist**
agent (decompose round) and the **gap-critic** agent (between rounds) follow. It is editable —
when a tactic stops working, change it here, not in code.

The binary executes whatever you decide via `research retrieve` flags:
`--domains a.gov,b.gov` · `--after MM/DD/YYYY` · `--before MM/DD/YYYY` ·
`--recency hour|day|week|month|year` · `--max-tokens-per-page N`. Verified live: these are the
**native** Search API params — use them, **not** inline `site:`/`filetype:` operators in the query
string (the API has real fields for this; inline operators are unreliable).

---

## 1. The one rule

**Decompose into specific, scoped sub-questions. Never fire "research everything about X."** A
broad query returns the SEO consensus — the same ten blog posts everyone else gets. A *specific*
query ("does a marketplace seller still owe sales tax in Texas when the platform already remits under the marketplace facilitator law") drags
up the page that actually answers it. Specificity is what separates this from a search box.

Front-load what you already know. The skill hands the strategist the question **plus the known
context** (the `../str` brief, the as-of date, the domain). Do not spend queries rediscovering
facts already in hand — spend them on the gaps.

---

## 2. The six decomposition axes

Every decomposition covers these axes. Not one query each — *coverage* of each. A standard run
fans 8–10 R1 queries across them; a max run 12–14. Tag every query with its axis so the triage and
gap stages can see what's covered and what's thin.

1. **Facet / sub-topic** — break the question into its component claims. "Is this SaaS viable?"
   decomposes into market size, regulatory forcing function, competition, willingness-to-pay,
   distribution. One query per load-bearing facet.
2. **Named-entity + geography** — pin the actual statutes, agencies, companies, places, people, bill
   numbers. "Texas marketplace facilitator sales tax permit requirement" beats "online sales tax rules." Names
   are how you reach primary sources.
3. **Time-window** — when recency matters, bound it. Use `--recency month` for fast-moving topics, or
   `--after 01/01/2025` to exclude stale consensus. Pair a "current state" query with a "what changed
   recently" query. Regulatory/news/markets always get a recency-bounded query.
4. **Source-type / primary-targeting** — aim queries straight at the primary tier with `--domains`.
   Legal → `--domains uscode.house.gov,ecfr.gov,courtlistener.com,regulations.gov` (or the state's
   `.gov`). Finance → `--domains sec.gov,fred.stlouisfed.org`. Health → `--domains
   pubmed.ncbi.nlm.nih.gov,fda.gov,cochranelibrary.com`. This is the highest-yield axis for
   authoritative work — it skips the commentary layer entirely.
   - **Academic breadth (escape the mainstream index):** the open web buries peer-reviewed primary
     literature. Reach it directly — `--domains scholar.archive.org,core.ac.uk,doaj.org,semanticscholar.org`
     (and `arxiv.org`, `pubmed.ncbi.nlm.nih.gov` for the fields they cover). These are *gateways*: the
     thing you actually **cite is the paper's DOI**, not the gateway or any access mirror. Never cite a
     piracy mirror URL — resolve to the DOI/publisher; the citation verifier flags mirrors and it's bad form.
5. **Contrarian / disconfirming** — at least one query per decomposition deliberately seeks the
   *opposite* of the expected verdict. "Why short-term-rental SaaS fails," "[product] criticism,"
   "[claim] debunked," "risks of [thing]." This is the anti-echo-chamber valve; without it you
   confirm your prior and call it research.
6. **Data / evidence** — target the numbers themselves: datasets, filings, official statistics,
   surveys, benchmarks. "[topic] statistics 2025 dataset," "[market] size report filetype" (via
   `--domains` to a stats agency, not inline filetype). Numbers anchor the synthesis; opinions don't.

---

## 3. Query craft — keywords and operators, not questions

Search engines rank on **keyword overlap and entities**, not grammar. Write queries the way a
librarian would, not the way you'd ask a person.

- **Keyword strings, not natural-language questions.** `Texas marketplace facilitator sales tax
  seller filing obligation remittance threshold` — not "Do online sellers in Texas have to file
  sales tax if the marketplace already collects it?" The bag of entities is what matters.
- **One natural-language "scouting" query is allowed** per decomposition — a single broad NL query to
  see what the landscape looks like and harvest vocabulary (the real statute names, the agency, the
  terms of art) for the operator-heavy queries that follow. After that, go specific.
- **Push filters into the native flags, never inline.** Want `.gov` only? `--domains x.gov`. Want
  2025+? `--after 01/01/2025`. Want the last month? `--recency month`. Do **not** write `site:` or
  `filetype:` in the query text — the Search API ignores/garbles them; its real params don't.
- **Scope tightly.** Each query should be answerable by a specific kind of page. If a query could
  return ten unrelated topics, it's too broad — split it.
- **Differentiate.** Before firing, check your query set for near-duplicates. Two queries that would
  return the same ten links waste a slot. Vary the entity, the angle, or the filter so each query
  reaches a *different* corner of the index. (The gap-critic re-checks this between rounds.)

---

## 4. Per-round doctrine

The engine runs up to three rounds. Each has a different job; the query style changes accordingly.

- **Round 1 — broad.** Cover all six axes. This is the wide net. Mandatory floor: **≥1 contrarian
  query and ≥1 primary-source (`--domains`) query** — non-negotiable, every decomposition. Grok-X
  fires at t=0 alongside R1 (separate modality, social signal as lead-gen).
- **Round 2 — gap-driven deepening.** The gap-critic reads the substrate against the gap-model and
  emits R2 queries that (a) **chase named primaries** the R1 reads pointed to — fetch the actual
  `.gov` statute / filing / legislative record, *not* another blog about it — and (b)
  adversarially re-test R1's strongest claims. R2 queries are narrower and more entity-specific than
  R1. **≥1 adversarial query** this round (and every round after R1).
- **Round 3 — narrow confirmation.** Pull the **3–6 load-bearing claims the verdict actually rests
  on.** Fire 5–10 *targeted* searches to independently re-confirm each a third time, from a different
  source than already used. Full-fetch the few new sources. This is narrow by construction — it does
  not re-broaden — so it adds rigor without runaway breadth. (This is the move that caught and
  confirmed SB 238 in the first live run, now automated.) Each load-bearing claim ALSO gets a
  **currency / supersession check** — recency-filtered queries (`--recency` / `--after`) keyworded for
  change (amended / repealed / overruled / retracted / superseded / update) — so a once-true claim that
  has since changed doesn't ship as current. This is where a stale verdict dies before the report does.

---

## 5. The query object — what the strategist emits

Each query is an object the workflow turns into one `research retrieve` invocation:

```
{
  "text":    "Texas marketplace facilitator sales tax permit registration requirements",
  "axis":    "primary-targeting",          // one of the six axes
  "domains": ["comptroller.texas.gov"],    // → --domains (optional)
  "after":   "01/01/2024",                 // → --after  (optional, MM/DD/YYYY)
  "before":  null,                          // → --before (optional)
  "recency": null,                          // → --recency (optional)
  "max_tokens_per_page": 1024,             // 1024 for broad/triage; 4096 when the
                                            // excerpt should yield shallow evidence
  "rationale": "trace the seller filing duty to the actual marketplace facilitator statute"
}
```

Defaults: omit a filter you don't need (the binary omits it from the request). `max_tokens_per_page`
defaults to 1024 — bump to ~4096 on gap/confirm queries and authoritative reads where a fat excerpt
can supply shallow secondary evidence without a full fetch.

---

## 6. Failure-mode defenses (hard rules)

These exist because each one is a way a deep-research run quietly goes wrong. They are not optional.

- **Mandatory contrarian + primary queries** (see §4 R1). The SEO trap is real: search the obvious
  phrasing and you get the obvious consensus. The contrarian query and the `--domains`-to-primary
  query are how you escape it.
- **Chase citations only from high-value reads.** When a strong, high-credibility source cites
  something, that's a lead worth a Round-2 query. When a thin blog name-drops a study, it is **not**
  — chasing every mention explodes the tree with low-value branches. Follow citations *down the
  credibility gradient deliberately*, not reflexively.
- **"An article about a study" is a LEAD, not a read target.** Never cite the news write-up of a
  filing/ruling/paper. Treat it as a pointer: extract the primary's identifier (DOI, docket, bill
  number, EDGAR accession) and fire a query to fetch the primary itself. Cite the primary.
- **≥1 adversarial query every round after R1.** Keep actively trying to break the emerging verdict.
  A verdict that survives repeated disconfirmation is strong; one that was never challenged is just a
  prior.
- **Track the provenance graph.** Know which claims rest on which tier. A verdict resting on a stack
  of tier 6–8 (blogs, social, forums) is Low confidence no matter how many of them agree — agreement
  among weak sources is often just one weak source echoed. Surface that in synthesis.
- **Heterodox ≠ authority.** The contrarian axis deliberately surfaces dissenting and independent
  sources — good. But anti-establishment doesn't mean right. Independent journalism with a track record
  is fine to weigh; conspiracy/pseudoscience (flagged `distrusted` in the binary's trust config, scored
  ~20) is included so synthesis *sees* the dissent but is **capped — never the sole basis for a
  load-bearing factual claim**, traced to primary like any social signal. On health, legal, and finance
  queries especially, don't let a low-credibility heterodox source drive the verdict.
- **No silent truncation.** When triage drops candidates (per-domain cap, pool ceiling, read cap),
  the workflow logs what was dropped and why. A run that quietly capped coverage reads as
  "comprehensive" when it wasn't.

---

## 7. Stopping observables (when a sub-question is done)

The read cap is a ceiling, not a target. Stop pursuing a sub-question **early** when:

- **≥2–3 independent, high-quality sources converge** on the same answer with no credible conflict.
  Independent = different origins/domains, not three pages citing one press release.
- The **primary source has been reached and read** (the statute itself, the filing itself) — there is
  nothing more authoritative to find; stop hunting commentary.
- A round of fresh queries returns **only sources already seen or strictly weaker** ones — the index
  is dry on this facet. (Two such dry rounds = stop the whole engine, not just the facet.)

Conversely, **do not stop** while: load-bearing claims still rest on a single source or on tier 6–8
only; sources actively conflict and the conflict is unresolved; or a named primary has been
identified but not yet fetched. Those are the gaps Round 2 exists to close.

---

## 8. Tier → query budget (set by the classifier; strategist picks within the band)

| Tier | R1 / R2 / R3 queries | Candidate pool | Full local reads |
|---|---|---|---|
| quick | 4–5 / — / — | ~70 | ~5 |
| standard | 8–10 / 5–7 / 5–7 (~22) | ~180 | ~14–16 |
| max (high-stakes) | 12–14 / 8–10 / 8–10 (~32) | ~280 | ~22–25 (hard cap 40) |

Candidate:read ratio ~10–12× — broad rounds run wide (8–15×), gap rounds tighter (4–8×), confirmation
tightest (2–4×). The strategist chooses the exact count *within* the tier's band from how many facets
the question actually has. A three-facet question doesn't need fourteen R1 queries; a sprawling one
might. Perplexity cost even at max ≈ $0.15 — irrelevant. The real budget is **full reads** (local
tokens/time), which is why the pool is wide and the read set is small and earned.
