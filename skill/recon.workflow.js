export const meta = {
  name: 'recon-deep',
  description: 'Iterative deep research: decompose into a wide query set, retrieve broad (Perplexity rounds + Grok-X at t=0), triage on excerpt, read the best survivors, close gaps, confirm the load-bearing claims, then synthesize a cited report and verify it. Wide-retrieve / selective-read; counts are disk-truthful.',
  phases: [
    { title: 'Decompose', detail: 'strategist: init run, 6-axis query set, gap model' },
    { title: 'Round 1', detail: 'broad retrieve (Perplexity ∥ Grok-X), triage on excerpt, read survivors' },
    { title: 'Round 2', detail: 'gap critic → gap-driven queries, primary-source tracing, read new' },
    { title: 'Round 3', detail: 'narrow confirmation of the 3-6 load-bearing claims' },
    { title: 'Synthesize', detail: 'cited report (writing contract) + verify-support/citations loop' },
  ],
}

// ---- inputs (from SKILL.md Step 4) ------------------------------------------
// { query, context, profile, tier, fast_sources, slow_sources, mode, as_of, skill_dir }
// The Workflow runtime may hand `args` to the script as a JSON *string* rather than a parsed
// object; normalize to an object so `args.query` resolves either way (otherwise every input
// reads undefined and the run decomposes the literal query "undefined"). `args` can be a
// read-only global, so parse into a local `A` instead of reassigning it.
const A = (typeof args === 'string') ? JSON.parse(args) : (args || {})
const query = A.query
const context = A.context || ''
const profile = A.profile || 'general'
const asOf = A.as_of || 'unknown'
const SKILL_DIR = A.skill_dir || '__RECON_SKILL_DIR__'

// Primary-tracing read-discipline is DECOUPLED from social suppression: a
// regulatory/health/finance-cored question can keep social ON (general/social
// profile) yet still demand statutes/filings/datasets be traced to the primary.
// The skill sets trace_primary for those; an authoritative profile always implies it.
const tracePrimary = A.trace_primary === true || profile === 'authoritative'

// The Perplexity key is read from the PERPLEXITY_API_KEY environment variable and passed
// through to each retrieve command's own process (never embedded in args/logs). Users who
// keep secrets in a manager can have it export the variable before launching.
const KEY = '${PERPLEXITY_API_KEY}'

// Connectors other than Perplexity, derived from the skill's routing split. These
// fire once on the scout query, concurrently with the Perplexity Round-1 loop
// (Grok-X at t=0). register_source locks, so the concurrent append is safe.
const fastExtra = (A.fast_sources || 'perplexity')
  .split(',').map((s) => s.trim().toLowerCase()).filter((s) => s && s !== 'perplexity')
const slow = (A.slow_sources || '')
  .split(',').map((s) => s.trim().toLowerCase()).filter(Boolean)
const extraSources = Array.from(new Set([...fastExtra, ...slow]))
// Routing guard (added 2026-06-11 after a live run silently lost Reddit): a SOCIAL profile
// is X + Reddit (sources.md routing), but the `grok` token is grok-X ONLY — the binary maps
// `reddit` as a SEPARATE connector (code/src/main.rs:303-305). So a caller that passes only
// `grok` for slow_sources quietly drops Reddit, the best raw-voice source for a sentiment
// question, with no error. Auto-add it so a fat-fingered token can't halve social coverage.
const autoAddedReddit = profile === 'social' && extraSources.includes('grok') && !extraSources.includes('reddit')
if (autoAddedReddit) extraSources.push('reddit')

// ---- tiers: query budgets, per-query limits, per-round read caps ------------
// Totals: quick ~5 reads / standard ~15 / max ~24 (hardCap is the safety ceiling).
const TIERS = {
  quick:    { r1: 5,  r2: 0, r3: 0, r1lim: 12, r2lim: 0,  r3lim: 0, r1read: 5,  r2read: 0, r3read: 0, hardCap: 8 },
  standard: { r1: 9,  r2: 6, r3: 6, r1lim: 15, r2lim: 12, r3lim: 8, r1read: 9,  r2read: 4, r3read: 2, hardCap: 20 },
  max:      { r1: 13, r2: 9, r3: 9, r1lim: 18, r2lim: 12, r3lim: 8, r1read: 14, r2read: 7, r3read: 3, hardCap: 40 },
}
const tierName = A.tier && TIERS[A.tier] ? A.tier : (A.mode === 'quick' ? 'quick' : 'standard')
const tier = TIERS[tierName]

// ---- schemas ----------------------------------------------------------------
const QUERY_ITEM = {
  type: 'object',
  required: ['text', 'axis'],
  properties: {
    text: { type: 'string' },
    axis: { type: 'string' },
    domains: { type: 'array', items: { type: 'string' } },
    after: { type: 'string' },
    before: { type: 'string' },
    recency: { type: 'string' },
    max_tokens_per_page: { type: 'integer' },
    rationale: { type: 'string' },
  },
}

const DECOMPOSE_SCHEMA = {
  type: 'object',
  required: ['run_dir', 'r1_queries', 'gap_model', 'verdict_question'],
  properties: {
    run_dir: { type: 'string' },
    r1_queries: { type: 'array', items: QUERY_ITEM },
    scout_query: { type: 'string' },
    gap_model: { type: 'array', items: { type: 'string' } },
    verdict_question: { type: 'string' },
  },
}

const RUN_SCHEMA = {
  type: 'object',
  required: ['ran'],
  properties: { ran: { type: 'integer' }, errored: { type: 'integer' }, note: { type: 'string' } },
}

const TRIAGE_SCHEMA = {
  type: 'object',
  required: ['selected'],
  properties: {
    selected: {
      type: 'array',
      items: {
        type: 'object',
        required: ['source_id'],
        properties: {
          source_id: { type: 'string' },
          raw_url: { type: 'string' },
          title: { type: 'string' },
          origin: { type: 'string' },
          category: { type: 'string' },
          why: { type: 'string' },
        },
      },
    },
    dropped: { type: 'integer' },
    notes: { type: 'string' },
  },
}

const READ_SCHEMA = {
  type: 'object',
  required: ['source_id', 'reachable', 'evidence_count'],
  properties: {
    source_id: { type: 'string' },
    reachable: { type: 'boolean' },
    evidence_count: { type: 'integer' },
    inaccessible: { type: 'boolean' },
    leads: { type: 'array', items: { type: 'string' } },
    contradictions: { type: 'array', items: { type: 'string' } },
    summary: { type: 'string' },
  },
}

const GAP_SCHEMA = {
  type: 'object',
  required: ['should_continue', 'r2_queries'],
  properties: {
    should_continue: { type: 'boolean' },
    remaining_gaps: { type: 'array', items: { type: 'string' } },
    r2_queries: { type: 'array', items: QUERY_ITEM },
    notes: { type: 'string' },
  },
}

const CONFIRM_SCHEMA = {
  type: 'object',
  required: ['load_bearing_claims', 'r3_queries'],
  properties: {
    load_bearing_claims: { type: 'array', items: { type: 'string' } },
    r3_queries: { type: 'array', items: QUERY_ITEM },
    notes: { type: 'string' },
  },
}

const SYNTH_SCHEMA = {
  type: 'object',
  required: ['report_path'],
  properties: {
    report_path: { type: 'string' },
    sources_total: { type: 'integer' },
    evidence_total: { type: 'integer' },
    claim_count: { type: 'integer' },
    factual_unsupported: { type: 'integer' },
    citation_issues: { type: 'integer' },
    limitations: { type: 'array', items: { type: 'string' } },
    remaining_gaps: { type: 'array', items: { type: 'string' } },
    summary: { type: 'string' },
  },
}

// ---- shell + command helpers ------------------------------------------------
// POSIX single-quote a value for safe interpolation into a shell command.
function shq(s) {
  return "'" + String(s).replace(/'/g, "'\\''") + "'"
}

// Build one `recon retrieve` invocation from a query object. The Perplexity
// key is substituted in-shell (never in args). domain/date/recency/excerpt-size
// go to the native flags, never inline operators.
function retrieveCmd(q, runDir, limit, sources) {
  let cmd = `PERPLEXITY_API_KEY="${KEY}" recon retrieve ${shq(q.text)}`
    + ` --mode deep --sources ${sources} --limit ${limit} --run-dir ${shq(runDir)}`
  if (q.domains && q.domains.length) cmd += ` --domains ${shq(q.domains.join(','))}`
  // Perplexity rejects --recency combined with --after/--before and requires
  // MM/DD/YYYY dates. Query agents sometimes emit both, or an ISO date — normalize
  // here so one malformed query can't 400 a whole round (battle-test finding 2026-06-11).
  const toMMDDYYYY = (d) => {
    const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(String(d).trim())
    return m ? `${m[2]}/${m[3]}/${m[1]}` : d
  }
  if (q.after || q.before) {
    // An explicit window is more specific than a relative recency — it wins; drop recency.
    if (q.after) cmd += ` --after ${shq(toMMDDYYYY(q.after))}`
    if (q.before) cmd += ` --before ${shq(toMMDDYYYY(q.before))}`
  } else if (q.recency) {
    cmd += ` --recency ${shq(q.recency)}`
  }
  if (q.max_tokens_per_page) cmd += ` --max-tokens-per-page ${q.max_tokens_per_page}`
  return cmd
}

// ---- prompt builders --------------------------------------------------------
function decomposePrompt() {
  return `You are the STRATEGIST for a deep-research run. Decompose the question into a wide, well-targeted query set, then output the plan as structured data.

Question: ${JSON.stringify(query)}
${context ? `Known context (front-loaded — do NOT spend queries rediscovering this):\n${context}\n` : ''}Profile: ${profile}   Tier: ${tierName}   Research-as-of: ${asOf}

First, read the query doctrine IN FULL and follow it: ${SKILL_DIR}/query-strategy.md

Then do all of this:
1. Create the run dir (prints {"run_dir":"..."} — capture the path):
   recon init-run ${shq(query)} --mode deep
2. Produce ${tier.r1} Round-1 Perplexity queries spread across the six axes (facet · entity+geo · time-window · primary-targeting · contrarian · data). MANDATORY floor: at least ONE contrarian/disconfirming query and at least ONE primary-source query (use the "domains" field to target .gov / filings / statute / primary domains). Write keyword strings + native filters, not natural-language questions (at most ONE scouting query may be NL). Push domain/date/recency into each query object's fields (domains / after MM/DD/YYYY / before / recency hour|day|week|month|year) — NEVER inline site:/filetype:. Set max_tokens_per_page to 1024 for broad queries, 4096 where a fat excerpt could itself yield shallow evidence.
3. Produce ONE scout_query: a concise keyword string for the non-web connectors (X / HN / markets).
4. Produce gap_model: 3-7 statements of "what must be true for the verdict to hold" — the things Rounds 2-3 exist to confirm. And verdict_question: the single sharpest framing of what we are deciding.

Return run_dir, r1_queries (array of query objects), scout_query, gap_model (array of strings), verdict_question (string).`
}

function retrieveLoopPrompt(queries, runDir, limit, roundLabel) {
  const cmds = queries.map((q, i) => `${i + 1}. ${retrieveCmd(q, runDir, limit, 'perplexity')}`).join('\n')
  return `You are a shell runner for ${roundLabel} web retrieval. Run EACH command below EXACTLY as written, in order, one at a time. Each appends into the shared run dir; the binary dedups and holds a lock, so sequential execution is correct. PERPLEXITY_API_KEY must be set in the environment for Perplexity queries to authenticate.

${cmds}

Each command prints JSON {run_dir,count,...} on success, or logs "source ... failed" to stderr on a connector error (non-fatal — keep going). Do NOT invent, reorder, or skip queries. Do NOT read or summarize the results — a later triage step reads them from disk. Return: ran (how many commands completed), errored (how many reported a connector error), note (one line).`
}

function extraRetrievePrompt(scoutQuery, sources, runDir) {
  const cmd = `recon retrieve ${shq(scoutQuery)} --mode deep --sources ${sources} --limit 12 --run-dir ${shq(runDir)}`
  return `You are a shell runner. Run EXACTLY this one command — it covers the non-web modalities (${sources}) concurrently with the web round. No Perplexity key is needed for these connectors. Grok-X can take 10-120s; wait for it to finish.

${cmd}

It prints JSON on success; a slow or failed connector logs to stderr (non-fatal). Do NOT summarize the results (triage reads them from disk). Return: ran (1 if the command completed, else 0), errored (count of connectors that failed per stderr), note (one line).`
}

function triagePrompt(runDir, gapModel, verdictQ, cap, excludeIds, roundLabel) {
  return `You are the TRIAGE pass for ${roundLabel}. Job: from the candidate pool on disk, select the BEST ${cap} sources to read in full. Search was wide and cheap; reading is dear — so keep the read set small and earned.

1. Get the pool from disk (disk-truthful — do NOT invent sources):
   recon list-sources --dir ${shq(runDir)}
   Returns {count, sources:[{source_id,raw_url,title,origin,date,score,recommendation,snippet}]} sorted by credibility. The "snippet" is a real page excerpt — use it to judge relevance and reject off-topic SECONDARY hits without fetching. EXCEPTION: never reject a SUBSTANTIVE primary for a blank/thin snippet — Perplexity returns an empty excerpt for PDFs and many .gov pages, so a missing snippet there means "no preview," not "off-topic" (see the primary read-floor below).
${excludeIds && excludeIds.length ? `2. EXCLUDE these already-read source_ids and triage only the rest: ${JSON.stringify(excludeIds)}\n` : ''}
Score each remaining candidate on: credibility (the binary's score), source-type (prefer PRIMARY — .gov/.edu, statutes, filings, datasets, primary-doc PDFs — over blogs/marketing), and gap-relevance to the verdict question + gap model:
  Verdict question: ${JSON.stringify(verdictQ)}
  Gap model: ${JSON.stringify(gapModel)}

Selection rules (NON-NEGOTIABLE):
- Cap at ${cap} survivors.
- Per-domain cap: at most 2-3 from any single domain (don't let one site dominate).
- Stratify across categories: official/primary · academic · quality journalism · analyst/industry · expert/practitioner · contrarian/critical · social.
- **Counter-evidence floor (NON-NEGOTIABLE):** the source that REFUTES a premise the verdict question assumes (a "no competitors", "settled", "only option" premise) is itself LOAD-BEARING — the verdict can't be trusted without it. A source that merely cuts against the pool's dominant signal (the hype / consensus / prevailing sentiment) qualifies ONLY if it brings specific contrary evidence on a gap axis from a source of at least mid credibility; mere disagreement, low credibility, or an off-topic gripe does not, and when the consensus is well-corroborated by primaries you already keep you need not invent a dissenter. If nothing in the pool genuinely refutes a premise or credibly cuts the signal, keep none — do NOT promote the closest available contrarian to satisfy this floor. When a source does qualify, judge it on WHAT it refutes and how on-point to a gap axis, not on its category, primary-ness, or a missing/thin snippet; keep the single most premise-relevant refuter within cap, ahead of a source that merely confirms what you've already kept.
- Prefer the primary source for any load-bearing or quantitative claim.
- **Primary read-floor (NON-NEGOTIABLE):** a candidate whose URL + title denote a SUBSTANTIVE primary artifact — a specific statute/filing/legislative-record/dataset page, a concrete official-doc page, a primary-doc PDF, or (for social) a specific thread/post carrying first-person testimony or a primary statement (not commentary on coverage available elsewhere) — is judged on its URL + title + the query axis that surfaced it, NOT on its snippet. Perplexity returns an empty/thin excerpt for exactly these (PDFs especially), so a blank snippet on a substantive primary is missing-preview, never low-relevance. NEVER drop a substantive primary for a missing/short snippet; fill the substantive-primary read-slots first, and if on-topic primaries exceed the cap keep the most gap-relevant ones (a relevant primary outranks an excerpt-rich secondary). When URL + title are too thin to classify a PDF or privileged-host document as substantive vs navigational, treat it as SUBSTANTIVE — ambiguity on a primary resolves to KEEP, never to drop. The floor does NOT shield a NAVIGATIONAL or PROMOTIONAL surface: a homepage, login/portal, vendor-marketing page, or SEO listicle loses the floor and competes on ordinary relevance like any secondary. Exception — these stay SUBSTANTIVE: an official statute/regulation index or table-of-contents page that is the canonical entry point to the primary text, and an official "overview"/"about" page that IS the substantive program document (not a generic capability or marketing surface).
- **Sentiment floor (NON-NEGOTIABLE; active only when the verdict question, in whole or in part, asks what some population thinks or feels):** for that part, that population's own first-person voices — specific independent threads/posts in their own words — ARE the primary evidence, and a piece ABOUT the sentiment is secondary, the way a statute outranks a blog about it (a poll/survey of that population is likewise primary — this ranks voices above commentary, not above measurement). So a raw voice outranks commentary-about-that-sentiment for the read-slots, and the per-domain cap here counts distinct authors/communities, NOT the shared platform host — neither the host-cap nor on-topic journalism-about-sentiment may crowd the raw voices out. Other-axis primaries the voices react to — a governing statute, an enforcement filing — still compete on their own gap-axis; this floor subordinates commentary-about-sentiment, not the factual anchors. Not a quota (cf. the counter-evidence floor): if the pool lacks credible on-topic voices, keep the few that exist and say so in notes — never promote spam or off-topic gripes to fill a count. On a purely factual/analytic question this elevation is INERT — a social post then earns a slot only as the read-floor's first-person primary, never as a sentiment sample.
- **Precedence when the cap binds:** if honoring every floor would exceed ${cap}, the order is (1) the single most premise-relevant refuter (at most one slot), (2) the most gap-relevant substantive primaries, (3) category stratification, (4) the per-domain cap — drop confirmers of what you already hold first. The per-domain cap binds over the floors EXCEPT it may never zero out the counter-evidence slot or the single most gap-relevant primary. In notes, say which floor you could not fully honor and why.

Return: selected (array of {source_id, raw_url, title, origin, category, why}); dropped (count not selected); notes (one line: pool size, how many dropped + main reason, any category that was thin/missing, and how many primaries were floor-kept). No silent truncation — the notes must own what was dropped.`
}

function readPrompt(s, runDir, verdictQ) {
  const authNote = tracePrimary
    ? '   PRIMARY-TRACING: if this is a secondary source ABOUT a primary one (statute/filing/ruling/study/dataset), do NOT treat it as the authority — record in "leads" the exact primary to fetch (bill number, docket, DOI, USC/CFR cite, EDGAR accession).\n'
    : ''
  return `Read ONE source in full and extract evidence bearing on the research question.

Question: ${JSON.stringify(query)}
Verdict question: ${JSON.stringify(verdictQ)}
Source title: ${JSON.stringify(s.title || '')}
URL: ${s.raw_url}
source_id: ${s.source_id}
run_dir: ${runDir}

Steps:
1. web_fetch the URL. If it is unreachable/blocked (403, paywall, anti-bot, timeout) AND this source is a primary or load-bearing, recover IN THIS ORDER:
   a. SCHOLARLY FULL TEXT (sci-hub) — only when this source is an academic paper carrying a DOI/PMID (origin openalex/crossref/pubmed, a doi.org / publisher URL, or a DOI/PMID visible in the URL or title). Fetch it: \`recon fetch-paper ${shq(s.raw_url)} --out ${shq(runDir + '/fulltext')}\` (it accepts a doi.org URL, a bare DOI, or pmid:NNN). On {found:true}, read pdf_path (pdftotext / the pdf skill) and use THAT as the content; tag evidence "provenance":"scihub_fulltext". CITE THE DOI/publisher in the bibliography, NEVER the mirror — pdf_path/domain_used are local read artifacts only, never a Source URL. A miss ({found:false} — e.g. a 2022+ paper outside the corpus, which is frozen at 2021) is EXPECTED: do NOT retry or loop, fall through to (b). Do this only for THIS load-bearing source — never fan out fetch-paper across many DOIs.
   b. GROK FETCH — only if a local \`grok\` CLI is installed (check \`command -v grok\`; a public install may not have it — if so, SKIP this rung and go to the inaccessible handling below). RETRY ONCE via Grok; its fetch infra reaches sources ours can't (old.reddit.com, some .gov/PDF hosts): run \`grok-ro -p "Fetch <URL> and return the full main text verbatim, no summary" --output-format plain --max-turns 4\`. Use the returned text as the content and tag any evidence drawn from it with "provenance":"grok_fetch".
   If the page is simply off-topic (you DID reach it, it just doesn't bear on the question), return reachable=false, evidence_count=0, inaccessible=false, no marker. If instead ALL recovery FAILED — no rung could obtain the text — on a source that MATTERED (a primary or load-bearing source, i.e. one you attempted recovery on above), return reachable=false, evidence_count=0, inaccessible=true, AND persist ONE gap marker so synthesis can't silently lose it — REGARDLESS of source type (not just academic papers). This is the ONLY non-quote row you ever write and it does NOT count toward evidence_count. Use the reason that fits:
      a. academic paper, sci-hub MISS ({found:false} — paywalled AND outside the 2021 corpus): \`recon add-evidence --dir ${shq(runDir)} --json '{"source_id":"${s.source_id}","quote":"[INACCESSIBLE — academic paper, full text paywalled and not retrievable: sci-hub miss (corpus frozen 2021) + Grok fetch failed; abstract/metadata only]","provenance":"inaccessible","evidence_type":"data_point"}'\`
      b. any other primary (statute / filing / .gov / PDF / dataset that blocked every rung): \`recon add-evidence --dir ${shq(runDir)} --json '{"source_id":"${s.source_id}","quote":"[INACCESSIBLE — primary source not readable: direct fetch blocked (403/paywall/anti-bot/timeout) + Grok fetch failed; not read]","provenance":"inaccessible","evidence_type":"data_point"}'\`
   Say so in the summary. NEVER fabricate content.
2. Extract 2-5 SHORT verbatim quotes (<=300 chars each) that directly bear on the question — facts, figures, findings, dates, named authorities, or clear positions. The quote field must be verbatim; no paraphrase.
3. Persist each quote (the binary fills the id + timestamp; minimal JSON is correct):
   recon add-evidence --dir ${shq(runDir)} --json '{"source_id":"${s.source_id}","quote":"<verbatim>","locator":"<section/para/page or the URL>","provenance":"primary_fetch","evidence_type":"direct_quote"}'
   Use single quotes around the JSON; if a quote contains a single quote, write the JSON to a temp file and pass it, or use printf — do not let shell quoting corrupt it.
${authNote}4. While reading, note leads and contradictions. For each lead that names a PRIMARY (bill number, docket, DOI, USC/CFR cite, EDGAR accession, dataset, official filing), format it as "PRIMARY: <identifier> — <why it matters>" so Round 2 can fetch that exact document directly; other follow-ups are plain strings. contradictions = claims here that conflict with another source or with the expected verdict.

Return source_id, reachable, evidence_count (how many quotes you actually persisted — a gap marker does NOT count), inaccessible (true ONLY when you wrote a gap marker in step 1, else false), leads (array), contradictions (array), and a one-line summary of what this source contributes. The count must reflect what you persisted, not what you read.`
}

function gapPrompt(runDir, gapModel, verdictQ, leads, contradictions) {
  return `You are the GAP CRITIC after Round 1. Decide what Round 2 must chase.

run_dir: ${runDir}
Verdict question: ${JSON.stringify(verdictQ)}
Gap model: ${JSON.stringify(gapModel)}

Read the substrate from disk: "recon list-sources --dir ${shq(runDir)}" for sources, and read ${runDir}/evidence.jsonl for the extracted quotes. Consult the doctrine: ${SKILL_DIR}/query-strategy.md (sections 4 Round 2, 6 defenses, 7 stopping).

Round-1 readers flagged these LEADS — turn every unfetched "PRIMARY:" lead into a Round-2 query that fetches the actual document (statute/filing/docket/DOI/dataset), NOT another blog about it:
${JSON.stringify(leads || [])}
...and these CONTRADICTIONS to resolve in Round 2:
${JSON.stringify(contradictions || [])}

Assess: which gap-model items are still unconfirmed, or rest only on weak (tier 6-8) or single sources? Which strong R1 claims deserve an adversarial re-test? Which named primaries (from the leads above) surfaced but were never fetched?

Emit up to ${tier.r2} Round-2 queries that (a) fetch the unfetched PRIMARY leads above and any other named primaries directly (use the domains field to target .gov / filings / primary domains) and (b) adversarially re-test the strongest load-bearing claims. At least ONE query MUST be adversarial/disconfirming. Keyword strings + native filters (domains/after/before/recency in the query object's fields), not natural-language questions, not inline site:/filetype:.

Return should_continue (set false ONLY if R1 already closed the gap model with strong, independent corroboration), remaining_gaps (array), r2_queries (array of query objects), notes (one line).`
}

function confirmPrompt(runDir, verdictQ) {
  return `You are the ROUND-3 CONFIRMATION strategist. The verdict will rest on a few load-bearing claims; set up an INDEPENDENT third confirmation of each (this is the move that catches a wrong-but-popular claim before it ships).

run_dir: ${runDir}
Verdict question: ${JSON.stringify(verdictQ)}
Read the substrate: "recon list-sources --dir ${shq(runDir)}" and ${runDir}/evidence.jsonl.

Identify the 3-6 claims the verdict most depends on — the ones that, if wrong, flip or gut the conclusion. For EACH, write narrow, specific queries that do BOTH:
 (a) CONFIRM the claim from a DIFFERENT, independent, high-credibility source than already used — ideally the primary itself; and
 (b) CHECK CURRENCY / SUPERSESSION as of ${asOf} — is the statute amended or repealed, the ruling overruled, the study retracted or corrected, the filing superseded, the figure now stale? Keyword the currency query for change ("amended", "repealed", "overruled", "retracted", "superseded", "update", "as of ${asOf}") and bound it with the recency/after fields (e.g. recency:"year" or after a recent date). A once-true claim that has since changed must not ship as current.
Total up to ${tier.r3} queries across all claims. Narrow confirmations + currency checks, NOT a re-broadening. Push domain/date/recency into each query object's fields.

Return load_bearing_claims (array of strings) and r3_queries (array of query objects), plus notes (call out any claim whose currency you could NOT confirm).`
}

function synthPrompt(runDir, verdictQ) {
  const authNote = tracePrimary
    ? '\n   - Trace every load-bearing claim to primary text; tag jurisdiction + as-of date; flag superseded/overruled/retracted material.'
    : ''
  return `Synthesize the final cited report from the evidence substrate, then verify it.

run_dir: ${runDir}
Question: ${JSON.stringify(query)}
Verdict question: ${JSON.stringify(verdictQ)}
Profile: ${profile}   Tier: ${tierName}   Research-as-of: ${asOf}
Writing contract + quality-tier ladder + primary-source hierarchies: ${SKILL_DIR}/sources.md

1. Read ${runDir}/sources.jsonl and ${runDir}/evidence.jsonl (use "recon list-sources --dir ${shq(runDir)}" for the credibility-sorted view). Triangulate across sources and modalities (web / social / repos / markets). Surface conflicts; do not average them away. Any evidence row with "provenance":"excerpt" rests on a Perplexity excerpt, not a full fetch — treat it as weaker and NEVER as the sole basis for a load-bearing claim. Any row with "provenance":"inaccessible" is NOT evidence — it is a GAP MARKER for a load-bearing PRIMARY (an academic paper, OR a statute/filing/.gov/PDF/dataset) that NO fetch rung could read (direct + sci-hub-if-academic + Grok all failed); never cite it as support (handle per the INACCESSIBLE PRIMARIES rule below). If a whole retrieval modality (X/social, HN, markets) returned nothing usable, SAY SO explicitly in the report — no silent omission.
2. Write a markdown report to ${runDir}/report.md per the writing contract in sources.md:
   - Lead with the verdict, then the reasoning. >=80% prose.
   - Immediate [N] citations at each claim; every [N] resolves to a real URL/DOI in a "## Bibliography" section (no ranges, no placeholders, no "various sources"). Label each source by quality tier and as primary / secondary / social.
   - No vague attribution ("studies show"); name the source. State confidence (High / Medium / Low / Unverified) on every load-bearing claim. Surface disagreements between sources explicitly.${authNote}
   - STEELMAN THE OPPOSITE VERDICT: before finalizing, state the strongest case AGAINST your conclusion and whether the evidence actually rules it out. If it does not, downgrade the verdict's confidence (or revise it) and say why. One tight paragraph bound to the actual load-bearing claims — not generic caveats.
   - INACCESSIBLE PRIMARIES: surface load-bearing PRIMARIES that could NOT be read in full (the "provenance":"inaccessible" markers — an academic paper outside the sci-hub corpus, OR a statute/filing/.gov/PDF/dataset that blocked every fetch rung), but AGGREGATE, never one line per source. Group by finding and give counts. If the accessible sources already converge on what an inaccessible source concerns, say so and HOLD confidence: "N primaries bearing on X could not be retrieved; the M I did read agree on X, so the gap doesn't change the verdict." If a load-bearing claim rests ONLY on inaccessible primaries, flag it explicitly, name each by its best identifier (DOI for a paper; the bill/docket/CFR cite or URL for a document), and downgrade it to Low/Unverified. One inaccessible primary → a single plain notice; many → the aggregated summary. Put this in the "what wasn't checked" note / ## Limitations.
   - CURRENCY: for every load-bearing legal/regulatory/health/finance claim, state its as-of-${asOf} validity (in force / amended / superseded / retracted / unconfirmed) and flag anything you could not confirm is still current.
   - Stamp the research-as-of date and add a short "what was and wasn't checked" note.
3. Persist the report's load-bearing FACTUAL claims (minimal JSON; the binary fills ids/timestamps):
   recon add-claim --dir ${shq(runDir)} --json '{"section_id":"<section>","text":"<claim>","claim_type":"factual","cited_source_ids":["<real source_ids backing it>"]}'
4. Verify and fix (up to 3 cycles):
   recon verify-support --dir ${shq(runDir)}
   recon verify-citations --dir ${shq(runDir)}
   If a factual claim is unsupported, or a citation is suspicious/unverified, fix the report (or downgrade the claim's confidence with a stated reason) and re-run. If still failing after 3 cycles, list them under "## Limitations".
5. Final disk counts: "recon list-sources --dir ${shq(runDir)}" for the source count; claim_count via "wc -l < ${runDir}/claims.jsonl". For evidence_total report REAL evidence only — EXCLUDE the inaccessible gap markers — by counting non-marker rows:  grep -cv '"provenance":"inaccessible"' ${runDir}/evidence.jsonl  (the markers are surfaced via the INACCESSIBLE PRIMARIES note, not the evidence count).

Return report_path, sources_total (disk), evidence_total (disk — real evidence only, excludes inaccessible markers), claim_count (disk), factual_unsupported (from verify-support), citation_issues (suspicious+unverified from verify-citations), limitations (array), remaining_gaps (array: the gaps STILL open AFTER synthesis — the report's "what wasn't checked" items, NOT the pre-round-2 gap model), summary (one line: the verdict + headline confidence).`
}

// ---- orchestration ----------------------------------------------------------
phase('Decompose')
log(`recon-deep: tier ${tierName}, profile ${profile}, extra connectors [${extraSources.join(',') || 'none'}]${autoAddedReddit ? ' (reddit auto-added: social profile)' : ''}`)

const decomp = await agent(decomposePrompt(), { schema: DECOMPOSE_SCHEMA, label: 'decompose', phase: 'Decompose' })
if (!decomp || !decomp.run_dir) {
  return { error: 'decompose failed (no run_dir)', run_dir: null }
}
const runDir = decomp.run_dir
const verdictQ = decomp.verdict_question || query
let gaps = decomp.gap_model || []
const r1Queries = decomp.r1_queries || []
log(`run ${runDir} — ${r1Queries.length} R1 queries, gap-model ${gaps.length} items`)

// ---- Round 1: broad retrieve (Perplexity loop ∥ extra connectors), triage, read
phase('Round 1')
const r1Thunks = [
  () => agent(retrieveLoopPrompt(r1Queries, runDir, tier.r1lim, 'Round 1'),
    { schema: RUN_SCHEMA, label: 'retrieve:pplx-r1', phase: 'Round 1' }),
]
if (extraSources.length) {
  r1Thunks.push(() => agent(extraRetrievePrompt(decomp.scout_query || query, extraSources.join(','), runDir),
    { schema: RUN_SCHEMA, label: `retrieve:${extraSources.join('+')}`, phase: 'Round 1' }))
}
await parallel(r1Thunks)

const tri1 = await agent(triagePrompt(runDir, gaps, verdictQ, tier.r1read, [], 'Round 1'),
  { schema: TRIAGE_SCHEMA, label: 'triage:r1', phase: 'Round 1' })
const sel1 = tri1 && tri1.selected ? tri1.selected : []
log(`Round 1 triage: ${sel1.length} selected to read (dropped ${tri1 ? tri1.dropped : '?'}) — ${tri1 ? tri1.notes || '' : ''}`)

const readIds = new Set(sel1.map((s) => s.source_id))
const reads1 = (await parallel(sel1.map((s) => () =>
  agent(readPrompt(s, runDir, verdictQ), { schema: READ_SCHEMA, label: `read:${s.origin || 'src'}`, phase: 'Round 1' })
))).filter(Boolean)
let totalReachable = reads1.filter((r) => r.reachable).length
let inaccessibleCount = reads1.filter((r) => r.inaccessible).length
log(`Round 1 read: ${totalReachable}/${sel1.length} reachable`)

// Readers flag leads (esp. "PRIMARY:" ones) + contradictions; collect them so the
// gap critic can act on them instead of re-deriving gaps from evidence.jsonl alone.
const leads1 = reads1.flatMap((r) => r.leads || [])
const contra1 = reads1.flatMap((r) => r.contradictions || [])

// ---- Round 2: gap critic → gap-driven deepening + primary tracing
let r2qCount = 0
if (tier.r2 > 0) {
  phase('Round 2')
  const gap = await agent(gapPrompt(runDir, gaps, verdictQ, leads1, contra1), { schema: GAP_SCHEMA, label: 'gap-critic', phase: 'Round 2' })
  if (gap) {
    gaps = gap.remaining_gaps && gap.remaining_gaps.length ? gap.remaining_gaps : gaps
    const r2Queries = (gap.r2_queries || []).slice(0, tier.r2)
    if (gap.should_continue !== false && r2Queries.length) {
      r2qCount = r2Queries.length
      await agent(retrieveLoopPrompt(r2Queries, runDir, tier.r2lim, 'Round 2'),
        { schema: RUN_SCHEMA, label: 'retrieve:pplx-r2', phase: 'Round 2' })
      const cap2 = Math.max(0, Math.min(tier.r2read, tier.hardCap - readIds.size))
      const tri2 = await agent(triagePrompt(runDir, gaps, verdictQ, cap2, [...readIds], 'Round 2'),
        { schema: TRIAGE_SCHEMA, label: 'triage:r2', phase: 'Round 2' })
      const sel2 = (tri2 && tri2.selected ? tri2.selected : []).filter((s) => !readIds.has(s.source_id))
      const reads2 = (await parallel(sel2.map((s) => () =>
        agent(readPrompt(s, runDir, verdictQ), { schema: READ_SCHEMA, label: `read:${s.origin || 'src'}`, phase: 'Round 2' })
      ))).filter(Boolean)
      sel2.forEach((s) => readIds.add(s.source_id))
      totalReachable += reads2.filter((r) => r.reachable).length
      inaccessibleCount += reads2.filter((r) => r.inaccessible).length
      log(`Round 2: ${r2qCount} queries, read ${sel2.length} new (reachable total ${totalReachable})`)
    } else {
      log('Round 2 skipped: gap critic judged Round 1 sufficient')
    }
  }
}

// ---- Round 3: narrow confirmation of the load-bearing claims
let r3qCount = 0
let loadBearing = []
if (tier.r3 > 0 && readIds.size < tier.hardCap) {
  phase('Round 3')
  const conf = await agent(confirmPrompt(runDir, verdictQ), { schema: CONFIRM_SCHEMA, label: 'confirm-strategist', phase: 'Round 3' })
  if (conf) {
    loadBearing = conf.load_bearing_claims || []
    const r3Queries = (conf.r3_queries || []).slice(0, tier.r3)
    if (r3Queries.length) {
      r3qCount = r3Queries.length
      await agent(retrieveLoopPrompt(r3Queries, runDir, tier.r3lim, 'Round 3'),
        { schema: RUN_SCHEMA, label: 'retrieve:pplx-r3', phase: 'Round 3' })
      const cap3 = Math.max(0, Math.min(tier.r3read, tier.hardCap - readIds.size))
      const tri3 = await agent(triagePrompt(runDir, loadBearing, verdictQ, cap3, [...readIds], 'Round 3'),
        { schema: TRIAGE_SCHEMA, label: 'triage:r3', phase: 'Round 3' })
      const sel3 = (tri3 && tri3.selected ? tri3.selected : []).filter((s) => !readIds.has(s.source_id))
      const reads3 = (await parallel(sel3.map((s) => () =>
        agent(readPrompt(s, runDir, verdictQ), { schema: READ_SCHEMA, label: 'read:confirm', phase: 'Round 3' })
      ))).filter(Boolean)
      sel3.forEach((s) => readIds.add(s.source_id))
      totalReachable += reads3.filter((r) => r.reachable).length
      inaccessibleCount += reads3.filter((r) => r.inaccessible).length
      log(`Round 3 confirmation: ${r3qCount} queries on ${loadBearing.length} load-bearing claims, read ${sel3.length} new (reachable total ${totalReachable})`)
    }
  }
}

// ---- Synthesize + verify
phase('Synthesize')
const synth = await agent(synthPrompt(runDir, verdictQ), { schema: SYNTH_SCHEMA, label: 'synthesize', phase: 'Synthesize' })

return {
  run_dir: runDir,
  report_path: synth ? synth.report_path : null,
  tier: tierName,
  queries_fired: { r1: r1Queries.length, r2: r2qCount, r3: r3qCount, extra: extraSources },
  sources_total: synth ? synth.sources_total : null,   // disk-derived
  sources_read: readIds.size,                           // tracked selections (not self-report)
  sources_reachable: totalReachable,
  sources_inaccessible: inaccessibleCount,              // primaries no rung could read — the silent-miss count, now surfaced (per-read flag, like sources_reachable)
  evidence_rows: synth ? synth.evidence_total : null,   // disk-derived, real evidence only (inaccessible markers excluded)
  claims: synth ? synth.claim_count : null,             // disk-derived
  verified: synth ? { factual_unsupported: synth.factual_unsupported, citation_issues: synth.citation_issues } : null,
  load_bearing_claims: loadBearing,
  // Truthful post-synthesis gaps; fall back to the R2 gap model only if synth didn't report.
  gaps_remaining: synth && synth.remaining_gaps && synth.remaining_gaps.length ? synth.remaining_gaps : gaps,
  limitations: synth ? synth.limitations || [] : [],
  summary: synth ? synth.summary : 'synthesis failed',
}
