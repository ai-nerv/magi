# Provider coverage plan

## Status and objective

- Status: **IN PROGRESS — parity harness, registry/readiness foundations and bounded discovery implemented; full parity is not achieved**.
- Planned on: **2026-11-20**.
- Inspected Magi commit: `e8c5bdd`.
- Inspected Melchior commit: `28937f8`.
- Scope: bring NERV's provider access up to a pinned snapshot of PI's provider coverage while preserving NERV's independent programs, Lua configuration, streaming contracts, accounting and containment.
- Pinned PI reference: **1.6.0**, commit `5a7479a88390d947335a1a6609936897be0a7891` (2026-11-20).
- Evidence so far: source inspection, isolated local HTTP fixtures and focused foundation tests. No live inference requests have been run. Some existing catalog-loading tests can fetch public metadata; isolated provider fixtures use disposable HOME/XDG directories. See the implementation checkpoint below for exact results and incomplete gates.

Adding a catalog entry is not completion. A supported integration needs usable authentication, correct routing, accurate model capabilities, working inference or classification, and verification evidence.

PI's inspected source registers **43 provider IDs**, including regional and subscription-plan variants. The initial Melchior baseline had **9 entries**; the current working tree has **34**, after adding Groq, Cerebras, NVIDIA, xAI, Meta, both MiniMax regions, Xiaomi PAYG plus three regional token plans, ZAI Coding China, Moonshot global/China, Kimi Coding, Baseten, Fireworks, four Qwen token plans, Together, Hugging Face, Ant Ling and Mistral. Thirty-two IDs now match PI; Ollama and OpenRouter Decisions are the two additional integrations. New declarations and model batches are not full provider qualification. The target remains all 43 reference IDs **plus preservation of our two extras**, not an arbitrary count of vendor names.

Implementation was subsequently requested as the active goal, "implement PRIVIDERS.md", referring to this file. Paid requests, changes to installed configuration, account login, deployment, commits and pushes remain unauthorized.

### Plan at a glance

1. **Target:** qualify all 43 pinned PI provider IDs and preserve `ollama` and `decisions`, for 45 integrations. Thirty-four provider declarations are bundled. Groq, Cerebras and NVIDIA cover four selected models; xAI adds four, Meta seven, MiniMax global/China four, Xiaomi PAYG/token plans nine, DeepSeek two, ZAI global/China eleven, Moonshot/Kimi eighteen, Baseten fifteen, Fireworks twenty-seven, Qwen plans sixty-six, Together sixteen, Hugging Face eighty-seven, Ant Ling three and Mistral thirty-four pinned chat records through API-key access. Their broader provider qualification remains open.
2. **Ownership:** implement provider access in Melchior. Casper owns tools, execution and MCP; Balthasar owns memory. Magi consumes neutral model/setup metadata and handles orchestration, selection and authentication presentation; it must not duplicate vendor clients. Adapter tool-call serialization only translates requests and results for the existing Casper execution path.
3. **Order:** prioritize Mistral next, as requested. Preserve the remaining registry/catalog, compatible API-key, authentication, cloud, special-service and end-to-end requirements. The phase table below records dependencies and remaining work.
4. **Hard parts:** credential renewal and account isolation; Bedrock EventStream/signing; Mistral Conversations; Azure mixed routing; Vertex credentials; trustworthy reasoning, tools, images and pricing metadata.
5. **Proof:** per-provider offline contracts, independent local gates, selected-family/UI/static-package checks, and separately authorized live receipts. The latest inspected parity report has **58 passed cases, 393 pending and zero live requests**. Those passes are generic text/usage protocol smoke cases, not complete provider-support counts; the new batch has additional native tests recorded below.

**Reading order:** use Section 3 for the complete inventory, Section 4 for ordered implementation tasks, Sections 5–6 for commands/tests, and Section 7 for the definition of done. Historical checkpoints below are evidence for their recorded source snapshots only.

**Current work — provider login UI (2026-11-21):** `:login` now opens the existing
centered-pane UI for masked API-key add/replace, saved-login removal and configured
authorization-code/PKCE browser sign-in. Mistral is listed first. Melchior owns the
bounded `auth --stdio` extension, private atomic credential storage and authoritative
readiness; Magi presents controls and cancels/reaps the model-role process. Casper's
tools/MCP and Balthasar's memory are unchanged. Keys do not enter prompt history,
observer events or snapshots. Environment keys retain precedence; local removal
does not revoke a vendor token. See [LOGIN.md](LOGIN.md) for usage and storage limits.
The bundled catalog has **no OAuth presets**: provider registrations, subscription
adapters, device flows, cloud credentials and full Phase 04 qualification remain open.
Melchior's full verification passes; Magi's 24 focused login/palette tests, full local
suite (2,923 passed, nine ignored), Clippy and all architecture gates pass. Actual
renderer fixtures at 80×24 and 160×28 were inspected.
Selected-family verification caught an obsolete command-palette snapshot after 715
passing tests and zero leaks; the fixture and a dedicated `:login` completion regression
now pass. The full rerun **passes: 6,966 tests passed, 16 ignored, zero failures and zero
leaked processes**, including all 24 login/palette cases. Canonical receipt:
`../target/family/run-qhFQ24Zm/summary.json`; retained login logs and rendered previews:
`target/login-verification/run-20261121-1MlPdWWh/`. The earlier failed receipts remain
intact. This is local/selected-family authentication acceptance, not provider subscription
parity, live-account qualification or static packaging. No real login or installation occurred.

**Previous work — Mistral adapter (2026-11-21):** the native `mistral-conversations`
dialect and all 34 pinned records are implemented in Melchior. It uses native
`/v1/chat/completions`, translates thinking chunks and collision-safe nine-character
call/result IDs, preserves ordered System updates, and reuses existing transport,
strict-schema validation, image bounds and credential-scoped cache identity. No tool
executor, MCP client, hosted connector or memory subsystem was added. This resumed
Mistral change touches no Magi runtime, Casper or Balthasar implementation.
Seven adapter tests, two compiler tests (272 model/effort bodies) and four native-process
tests (342 loopback HTTP requests) pass, including local cancellation, pre-authentication
refusals, native call/result history, capability listing, image fallback and usage.
Session/image/protocol regressions, Chat Completions regressions, all 45 catalog tests,
exact generation, Clippy and seven architecture gates pass. Full native verification
also passes: 1,033 library and 165 readiness tests in default/all-feature runs, followed
by hermetic, native family/model-role and dependency/security gates. Two obsolete
adapter inventories were updated; a reproduced off-reasoning budget defect is fixed
and covered by a regression. Coverage is **34/45 declarations**, **307 selected imports** and
**1,183/1,779 complete candidate shapes**. Actual Mistral Magi/Casper host execution,
full family/UI/static-package acceptance and live account/model qualification remain
open. Nothing was installed or deployed. Evidence:
`target/provider-foundations/mistral-20261121T152137Z-n4kuGChT/`.

**Previous work — Google tool-call fidelity (2026-11-21):** four reproduced adapter failures
now pass after preserving distinct provider IDs, generating collision-free fallback IDs
and pairing modern-model call/result history. A native regression also reproduced lossy
JSON argument decoding; exact-value decoding now preserves nulls, empty containers and
unsigned integers. Five adapter tests and one native test (24 HTTP requests) pass, along
with streaming/usage regressions, Clippy and all native architecture gates. Ordered signed
replay now also passes seven adapter regressions and actual Magi-host execution: two APIs,
eight scenarios, 30 loopback requests and eight Echo calls, including JSON/CBOR history
restoration, origin changes and retry cleanup. Full native verification passes.
A reproduced pending-queue defect omitted opaque signature bytes from its memory bound;
the fix passes all three pending-memory regressions. The post-fix host replay rerun,
Clippy and all eleven Magi architecture gates pass; sequential full Magi verification
also passes. Retained debug/release host regression checks pass.
The first full native run found an obsolete image-fixture call ID after 1,012 library /
157 readiness tests passed; the exact fixture expectation is repaired and verification
passes on its rerun, including the repaired fixture. These changes postdate the
frozen Ant Ling source below; full provider qualification remains open.

**Subsequent Google accounting fix (2026-11-21):** six reproduced adapter failures now
pass after including reported thought tokens in output usage. Two native tests add ten
loopback requests proving exact late-terminal pricing and separately charged partial
retries. Usage/accounting/spending regressions, Clippy and native architecture gates pass.
Candidate/thought snapshots are independently monotonic; invalid and missing counters
retain distinct evidence. Source and focused logs are retained under
`target/provider-foundations/google-usage-20261121T141936Z-A7L4jTyh/`.
An actual host regression first reproduced 12 output tokens instead of 19 against the
retained pre-fix image. The updated image passes all eight host scenarios / 30 requests,
including exact thought-inclusive usage and prices on success, length, error and paid
retry; Magi Clippy and all eleven architecture gates pass. This fix postdates the replay
snapshot's full native verification. Full updated-source native verification now passes
(1,018 library / 159 readiness tests in default/all-feature runs), as does sequential
full Magi verification. Both retained debug/release images pass Google replay and
Completions host checks. Discrete reasoning controls and broader provider qualification
remain separate work; this snapshot does not contain the subsequent control resolver.

**Subsequent Google reasoning controls (2026-11-21):** the shared resolver now distinguishes
discrete levels from model-specific token budgets. Model menus and requests share the same
supported controls; explicit null/empty restrictions, mandatory reasoning, uppercase Gemma
maps and real Hydrogen/Neon `xhigh` are retained. Seven adapter, two compiler and two native
tests pass: all 53 pinned Google-API records / 424 in-process requests, plus 128 loopback
requests. Six formerly pending shapes now convert, for **1,149/1,779 complete shapes**;
declarations and selected imports remain **33/45 and 273**. The first rounded run stopped
at a test-only Clippy boolean simplification after all focused/catalog/import tests passed.
That fixture is repaired; thinking/usage/replay/call regressions, Clippy and all seven
native architecture gates now pass. Actual Magi-host execution also passes **144 requests
across two APIs / nine profiles**, plus the existing eight replay/usage scenarios. Magi
Clippy and all eleven architecture gates pass. The first host fixture lost empty-array
shape through the legacy JSON decoder; using the existing exact decoder repairs the
fixture without relaxing assertions. Full current-source native verification passes
(1,025 library / 161 readiness tests in default/all-feature runs, plus later gates).
Sequential full Magi verification also passes. Both retained debug/release native images
pass the reasoning, replay and Completions host recipes against the unchanged retained
host executable. Evidence: `target/provider-foundations/google-thinking-20261121T145151Z-pQ0vmNYU/`.
No new Google/Vertex/OpenCode records are activated by this data-shape improvement.

**Latest verified batch — Ant Ling activation (2026-11-21):** three pinned models are bundled,
bringing the working tree to **33/45 declarations / 273 selected imports / 1,143 complete
candidate shapes**. A typed mapped-effort dialect supports Ring's `high`/`xhigh` without
inventing `max` or enabling top-level effort controls. Fourteen focused tests and actual
Magi-host debug execution pass. Retained debug/release host regressions also pass.
Full Melchior verification passes with 1,000 library / 156 readiness tests in default and
all-feature runs. Full Magi verification passes on a sequential rerun. Its first run
failed because the shared-runtime hermetic check detected concurrent Melchior test files;
the gate was not weakened and ambient files were not deleted. The complete-parity gate
correctly fails at 58 generic passes / 393 pending / zero live requests.
Evidence: `target/provider-foundations/ant-ling-activation-20261121T131826Z-xxVlQzx3/`.
This is an offline batch checkpoint, not full provider or live/static acceptance.

**Previous verified batch — Hugging Face activation (2026-11-21):** three compiler regressions
reproduced missing implicit PI defaults. A reviewed provider/API/URL-scoped profile now
preserves effort support, `store=false` and `max_completion_tokens` without changing
global runtime defaults. All 87 records convert, including six previously pending `xhigh`
models; 696 in-process request checks pass. Complete shapes rise to **1,140/1,779**.
All 87 records are now explicitly selected, bringing the working tree to **32 declarations /
270 selected imports**. Eleven focused tests pass, including **1,659 native loopback
requests**, 261 pre-auth refusals and one unknown-model refusal. Retained debug and release
Magi-host runs each pass **87 models / 174 scenarios / 522 requests / 348 Echo calls**,
plus all earlier host batches. Full Magi and Melchior verification pass; Melchior runs
996 library / 148 readiness tests in default/all-feature profiles and passes all later gates.
The first activation fixture incorrectly required source iteration order and treated unknown
prices as free; corrected expectations preserve exact membership and nullable accounting.
Evidence: `target/provider-foundations/huggingface-activation-20261121T125618Z-p4EzwCBH/`.
The complete-parity gate correctly fails at 56 generic passes / 395 pending / zero live
requests. This is an offline batch checkpoint, not full provider or live/static acceptance.

**Previous verified batch — Together (2026-11-21):** all 16 pinned models are bundled. Coverage is
**31/45 declarations**, **183 selected imports** and **1,134/1,779 complete shapes**.
Fourteen focused tests pass: four adapter, two compiler and eight native. Native coverage
includes **310 loopback requests**, 48 pre-auth refusals and one unknown-model refusal.
Actual Magi-host execution passes 16 models / 32 scenarios / 96 requests / 64 Echo calls,
including both reasoning-field spellings and all nine vision models. Full Magi and Melchior
verification pass; Melchior runs 996 library / 140 readiness tests in default/all-feature
profiles plus all later gates. Retained debug/release hosts pass Together and all prior
batches. The complete-parity gate correctly fails: 54 generic passes / 397 pending / zero
live requests. Evidence: `target/provider-foundations/together-activation-20261121T122802Z-6la1djtL/`.
This is offline batch evidence, not full provider qualification or live/static acceptance.

**Previous frozen checkpoint — Qwen activation (2026-11-21):** all **66 records** across four
team/individual/global/China variants are bundled. Coverage is **30/45 declarations**,
**167 selected imports** and **1,119/1,779 complete shapes**. Model-supported efforts and
explicit `xhigh` reach requests without changing binary/null/unspecified controls.
Reviewed provider defaults use `max_completion_tokens`; primary/fallback key order and
regional endpoints match the pin. Fifteen focused tests pass (four adapter, two compiler,
nine native): **1,092 loopback requests, 198 pre-authentication refusals and four unknown-model
refusals**. These include images, parallel tools, later-user reasoning, tool updates,
retries, malformed/auth errors and cancellation of every native model after partial usage.
Evidence: `../melchior/target/provider-foundations/qwen-activation-rounded-2.log`.
Actual Magi-host execution passes all **66 models / 198 requests / 132 Echo executions**,
including image adaptation for 36 vision models and later-user reasoning replay:
`target/provider-foundations/qwen-host-debug.log`. Full Magi verification passes; this run
does not establish a nine-case editor/visual skip count. Final Melchior activation verification passes: 992 library
and 132 readiness tests in both default/all-feature runs, followed by hermetic cleanup,
architecture, family/role, lint and dependency gates. Retained debug and release host runs
each pass all 66 Qwen models plus Fireworks, Moonshot/Kimi, Baseten and generic regressions.
The complete-parity gate correctly fails with 52 generic passes / 399 pending / zero live
requests. Evidence: `target/provider-foundations/qwen-activation-20261121T120542Z-hyan2iJZ/`.
This is offline batch evidence, not full qualification, static packaging or live acceptance.
Together dialect work started after this source snapshot and its full verification.
The unsupported inherited skip-count wording in the frozen receipt is clarified in
`target/provider-foundations/qwen-activation-20261121T120542Z-hyan2iJZ-errata.md`.

**Previous frozen checkpoint — Fireworks (2026-11-21):** all 27 pinned models are bundled
(16 Anthropic Messages / 11 Chat). Coverage is **26/45 declarations**, **101 selected
imports** and **1,107/1,779 complete shapes**. The offline batch is verified; full provider
qualification remains open.
Typed native Anthropic schemas preserve reasoning and ordinary tools; protocol-specific
affinity and tool-cache/eager controls preserve existing defaults. A reproduced usage
bug is fixed: final Anthropic input/cache counts no longer remain at their initial zeros.
Seventeen focused tests pass (seven adapter, two compiler, eight native), covering 366
loopback requests and 124 pre-authentication refusals, including images, parallel tools,
unsigned reasoning replay and positional/folded tool updates, plus one unknown-model
refusal. Catalog/exact generation, importer and lint checks also pass. Evidence:
`../melchior/target/provider-foundations/fireworks-activation-qualified.log`.
The foundation verification run completed, but activation began near its end; it is not
final batch evidence. The new actual-host image test exposed missing worker dispatch in
the example; it now uses the existing bounded image worker. All 27 models pass against
the retained debug image: **81 requests / 54 actual Echo executions**, with images and
later-user reasoning replay. Evidence: `target/provider-foundations/fireworks-host-debug-2.log`.
The retained release image also passes all 27 models / 81 requests / 54 Echo executions.
Both images pass the existing host, Moonshot/Kimi and Baseten regressions. Full local
verification passes in both checkouts: Melchior has 988 library / 123 readiness tests
in default and all-feature runs; Magi retains nine explicit editor/visual skips.
The complete-parity gate correctly exits 1 with **44 generic smoke passes / 407 pending**.
Images, logs, all 43 source inputs, extracted/hash-checked archives and final docs are
retained in `target/provider-foundations/fireworks-20261121T112521Z-xFYlVWg4/`.
No independent archive rebuild, static-package, full CLI/family/UI or live proof is claimed.

**Previous frozen checkpoint — Baseten (2026-11-21):** all 15 pinned models are selected, including both GLM 5.2 template-controlled models. Typed `chat_template_args` preserves boolean/variable controls and rejects unsupported shapes. Neutral off is distinct from its mapped `none` value; Kimi K3.1's pinned no-effort contract advertises an empty menu. Coverage is **25/45 declarations**, **74 selected imports** and **1,082/1,779 complete shapes**. Thirteen focused tests pass, including eight native tests with 276 loopback requests, 75 pre-authentication refusals and one unknown-model refusal. Both full local verification suites pass; Melchior has 980 library / 115 readiness tests in default/all-feature runs, and 30 catalog tests/exact generation plus importer tests pass. Actual Magi-host execution against debug and retained-release Melchior passes all 15 models / 45 requests / 30 Echo executions, plus Moonshot/Kimi and existing host regressions. Evidence: `target/provider-foundations/baseten-20261121T103958Z-2erq7QOK/`. This is offline batch acceptance, not full provider qualification or completed parity.

**Previous frozen checkpoint — session/cache identity:** **24/45 declarations**, **59 selected imports**, **1,080/1,779 complete shapes**. Both repositories' local verification suites pass, including Magi's four identity units/four integration tests and the wire-gate regression. Actual host runs against retained debug/release Melchior pass all 18 Moonshot/Kimi models / 54 requests / 36 Echo executions and the existing 14-scenario regression. Source archives, hashes, images and logs are retained in `target/provider-foundations/session-20261121T100747Z-PfARstSj/`. This checkpoint predates Baseten; no archive rebuild, full production CLI/memory restart, static packaging, UI, full-family or live acceptance is claimed.

**Earlier frozen Moonshot checkpoint:** **24/45 declarations**, **59 selected imports**, **637/1,779 complete candidate shapes**. Moonshot global/China and Kimi Coding add 18 pinned models. Eleven focused tests pass, including eight native cases with 264 loopback requests, 36 pre-authentication refusals and three unknown-model refusals. Full Melchior verification passes (971 library / 101 readiness tests in default and all-feature runs); all 25 catalog tests and exact generation pass. Full Magi verification passes with nine existing editor/visual skips. Actual Magi host execution passes all 18 models / 54 requests / 36 Echo executions against debug, selected release and retained-copy Melchior; the existing 14-scenario host regression also passes. Evidence: `target/provider-foundations/moonshot-20261121T093425Z-1B6Vx0nm/`. Authentication/cloud/missing protocols, production/durable System and tool-history generation, provider qualification, full CLI/memory restart, selected-family/UI/static/live acceptance and the earlier whole-family Casper failure remain open. Kimi OAuth and account entitlement remain separate requirements; the session work above postdates this checkpoint. Four MiniMax M2.7 records, retired Ant Ling source records, ZAI strict-tool guarantees and Coding Plan eligibility are not removed from scope.

**Previous frozen checkpoint:** `target/provider-foundations/tool-history-20261121T084823Z-bczIjUhS/` records 21 declarations / 41 imports, 36 focused tool-history tests, full Melchior verification (969 library / 93 readiness), full Magi verification with nine existing editor/visual skips, and 14 selected/retained-copy host scenarios / 28 requests / eight Echo executions. Its complete-parity gate correctly failed with 34 generic smoke passes / 417 pending / zero live requests.

## 1. Architecture to preserve

**Newest implementation:** Ant Ling adds explicit-only nested reasoning efforts and three pinned models. Hugging Face adds 87 router records with reviewed, model-local PI defaults. Together adds typed hybrid reasoning controls while its GPT-OSS record keeps the existing effort-only dialect. Four Qwen token plans reuse Chat with model-specific efforts and reviewed output caps. Fireworks reuses Chat and Anthropic adapters, with native schemas, final usage updates and protocol-specific affinity. Baseten retains typed reasoning-template controls. Moonshot/Kimi retain exact imports and K2.6 preserved thinking. Neutral `Content::ToolChanges` blocks preserve bounded state replay and opted-in positional additions. Independent wire definitions remain checked by the twin gate; no shared crate or Magi vendor client is added. Native fixtures prove local dispatch, not production generation/durable replay, full CLI/UI behavior or remote entitlement. Coverage is 33 declarations, not 33 fully qualified providers.

```text
Magi: model selection, auth presentation, session controls
    |
    | provider-neutral models / ask / optional auth extension
    v
Melchior: provider catalog, model metadata, credentials, adapters, transport
    |
    +-- Lua: provider declarations and protocol conversion
    +-- Rust: bounded I/O, credential lifecycle, signing, stream framing
    +-- existing usage, retry, overflow and spending machinery
```

- Magi must not acquire a second provider registry or vendor-specific HTTP clients.
- Melchior owns provider credentials and network integration. Casper's remote-tool OAuth implementation is a different responsibility; do not move model credentials into Casper.
- Keep the four programs independently buildable. Do not introduce a shared Rust crate between Magi and Melchior to avoid maintaining their wire definitions.
- Keep registration-style Lua configuration: `melchior.provider(...)` and `melchior.api(...)`. Reuse the existing shipped/config/plugin/after layering and installed-plugin acknowledgement rules.
- Keep protocol reuse. An additional vendor usually needs a declaration and tested compatibility settings, not a copied streaming implementation or another SDK.
- Preserve terminal stream completion, cancellation, bounded resources, retry classification, usage accounting and run-wide spending limits.
- Auth is an optional, discoverable extension of the model role unless a separately reviewed protocol revision makes it mandatory. Existing replacement model programs implementing `models` and `ask` must continue to work.

### Scope boundaries

Paths below are relative to the Magi checkout containing this file.

**In scope for the authorized implementation:**

- `../melchior/config/{providers,apis}.lua`.
- `../melchior/src/mind/`, including provider, catalog, discovery, authentication and wire types.
- `../melchior/src/main.rs` and relevant CLI dispatch for provider/auth commands.
- `../melchior/tests/`, `../melchior/examples/`, and provider-related scripts.
- `crates/magi-proto/`, `crates/magi-model/`, `crates/magi-host/`, `crates/magi-cli/`, and targeted model/auth UI code in `crates/magi-tui/`.
- `crates/magi-core/src/turn.rs` only for preserving opaque provider continuation state when no visible thinking text accompanies it; tool dispatch and turn policy remain unchanged.
- Corresponding tests, wire gates and `.make.lua` recipes in both repositories.
- `crates/magi-lua/src/tool.rs`, `crates/magi-tools/src/{registry,command,schema}.rs` and strict-schema helper/tests only for opt-in provider-tool sampling declarations and optional-null argument handling; tool permission and containment policies remain unchanged.
- `crates/magi-tools/src/family.rs` only for the reproduced test-scratch lifecycle defect exposed by the mandatory hermetic gate; no tool policy changes.
- `../scripts/` and `../.make.lua` only for selected-binary family/provider acceptance.
- Provider-facing README/EXTENDING/ROLES material inside Magi and Melchior, and this plan.
- Cargo manifests/lockfiles and existing Nix packaging only when required for reviewed provider dependencies; preserve the current static-build contract.

**Out of scope:**

- `../site/`, `../docs/`, website generators and website-specific Node tooling.
- General Casper or Balthasar refactors, tool permission changes, or memory architecture changes.
- Replacing NERV with PI, embedding PI's Node runtime, or making a proxy service a mandatory dependency.
- Full PI feature parity beyond provider access: image generation, audio generation and unrelated extension/UI features are not implied. Image input and tool-result images remain required where advertised. Direct TypeSafe classification is explicitly included below.
- Reading unrelated application credentials, copying another application's auth store, modifying real HOME/XDG data in tests, installing binaries or silently logging in.

Workspace permissions were expanded after planning and now allow the scoped sibling and family-root changes above. Website exclusions and independent repository boundaries remain unchanged.

## 2. Baseline and concrete gaps

This table and the excerpts below describe the initial inspected commits, not the current dirty worktrees. Some gaps have since been addressed by the implementation checkpoints in Section 4. Reinspect the current source before applying a fix; do not reintroduce a baseline defect to match an excerpt.

| Area | Current state and evidence | Required outcome |
|---|---|---|
| Provider catalog | Nine declarations in `../melchior/config/providers.lua:13–135` | Reviewed reference matrix, maintained metadata, preserved existing IDs |
| Protocols | Nine registered Lua adapters; ten `Api` variants, including unimplemented Bedrock | Implement missing required dialects; distinguish known, registered and usable |
| Mixed APIs | `catalog.rs:172–177` preserves an explicit model API, but `provider/endpoint.rs:182–184` overwrites it | Explicit model API wins; provider API is only a default |
| Extension validation | `provider/model.rs:12–36` is a closed enum; `catalog.rs:95–99,181` silently drops failed declarations | Clear diagnostics and validated protocol selection |
| Auth | API-key environment lookup and generic PKCE/refresh exist; AWS SigV4 and Google ADC are stubs in `provider/endpoint.rs:52–77` | Provider-specific auth strategies and real cloud credentials |
| Readiness | `catalog.rs:237–244` always marks OAuth/cloud cards unready | Secret-free readiness from Melchior, including usable stored OAuth credentials |
| Magi readiness | `crates/magi-proto/src/ask.rs:47–50` carries one variable; `magi-host/src/environment.rs:76–79` infers readiness locally | Structured auth/setup actions; refresh authoritative state after changes |
| Discovery | `discovering.rs:36–79,175–210` is sequential and assumes `/models`, bearer auth and `{data: [...]}` | Explicit discovery strategies, bounded concurrency and offline catalog |
| Discovery cache | `discovering.rs:99–111` keys only by flattened provider ID | Collision-resistant endpoint/account-aware cache namespaces |
| Azure | `config/apis.lua:524–533` hardcodes a deployment URL and preview API version | Reviewed current routing, version and deployment configuration |
| Vertex | `config/apis.lua:937–949` already converts Google requests | Complete auth and endpoint configuration, not a second Gemini implementation |
| Bedrock | `lua/adapter/mod.rs:334–341` explicitly identifies missing binary framing | Bounded AWS EventStream transport and Converse conversion |
| Magi catalog limits | ACP settings cap models at 4,096 and about 1 MiB; broker refresh has a 30-second timeout | Scale within explicit bounds; never remove limits to make tests pass |

### Drift-check excerpts

Initial API overwrite in `../melchior/src/mind/provider/endpoint.rs`:

```rust
for model in &mut models {
    model.provider.clone_from(&id);
    model.api = api;
    model.compat = Some(model.compat.unwrap_or_default().over(compat));
}
```

Initial readiness in `../melchior/src/mind/catalog.rs`:

```rust
let ready = match &provider.auth {
    Auth::ApiKey { vars } => vars
        .iter()
        .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty())),
    Auth::None => true,
    _ => false,
};
```

Existing provider convention, from `../melchior/config/providers.lua`:

```lua
melchior.provider("deepseek", {
  name = "DeepSeek",
  api = "openai-completions",
  base_url = "https://api.deepseek.com",
  auth = { kind = "api-key", vars = { "DEEPSEEK_API_KEY" } },
  compat = { thinking_format = "deepseek" },
  models = {
    -- Existing model declarations supply limits and capabilities.
  },
})
```

Match the existing `Result`/typed-refusal style, Lua adapter layout and fixture structure. New comments describe behavior only; no implementation-history or justification comments. Keep comments within the user's 20% limit.

## 3. Reference provider matrix

The integration groups below are work assignments, **not assertions that every member speaks identical HTTP**. Phase 00 must read each provider's pinned definition and authoritative service documentation, recording the actual protocol, auth methods and endpoint rules before implementation.

Groups:

- **R — retain and qualify:** shipped today; inspect current upstream behavior and run the same contracts as new entries.
- **K — compatible API-key batch:** start from existing adapters; add only verified compatibility differences.
- **A — subscription/auth integration:** complete provider-specific credential lifecycle and discovery.
- **C — cloud integration:** resource/account configuration and cloud auth or signing.
- **P — missing protocol:** actual conversion or transport implementation.
- **S — specialized service:** gateway/catalog or classifier behavior requires explicit qualification.

The table reflects current declarations and selected offline batches. Every row remains
unqualified for complete provider/live parity; existing source is not live acceptance evidence.

| # | PI provider ID | Current implementation | Work group / qualification focus |
|---|---|---|---|
| 1 | `amazon-bedrock` | No provider; protocol enum only | C/P: credentials, signing or supported bearer auth, EventStream, Converse |
| 2 | `ant-ling` | Three pinned models; focused, retained-host and full local checks pass | K: explicit-only efforts, tools, text-only image fallback; full provider/live qualification pending |
| 3 | `anthropic` | Shipped | R: API key and documented alternative auth, cache/reasoning/tool replay |
| 4 | `google` | Shipped; calls/replay/usage verified offline, reasoning foundation in progress | R: qualify full pinned catalog, model-specific behavior and durable restart |
| 5 | `google-vertex` | Adapter only; ADC stub | C: project/location, supported key auth, ADC and refresh |
| 6 | `openai` | Shipped | R: Responses, current model metadata and capabilities |
| 7 | `azure` | Azure Responses adapter only | C: resource/base URL, deployment mapping, version, mixed APIs |
| 8 | `openai-codex` | Adapter only; no shipped auth preset | A: supported login, token/account metadata, subscription request behavior |
| 9 | `radius` | No provider; Pi Messages adapter exists | A/S: gateway auth, catalog and endpoint-specific behavior |
| 10 | `typesafe` | Indirect OpenRouter Decisions integration only | S: direct classifier authentication, schema and result contract |
| 11 | `nvidia` | One pinned model selected | K: remaining NIM records and full qualification |
| 12 | `deepseek` | Two pinned models selected | R: full reasoning/tool/metadata qualification |
| 13 | `github-copilot` | Device sign-in, renewal, account endpoint/filtering; 14 selected imports plus legacy entry | R/A remaining: broader model mappings, custom expiring GitHub credentials, live account qualification |
| 14 | `xai` | Missing at baseline; now four pinned API-key models | K/A: Responses batch tested offline; subscription OAuth and full qualification pending |
| 15 | `groq` | Two pinned models selected | K: remaining records and full Chat-compatible qualification |
| 16 | `cerebras` | One pinned model selected | K: remaining records and full Chat-compatible qualification |
| 17 | `openrouter` | Shipped with discovery | R/S: key-aware catalog, routing restrictions and billed usage |
| 18 | `vercel-ai-gateway` | Missing | K/S: gateway routing, headers and model metadata |
| 19 | `zai` | Shipped coding endpoint; now seven pinned models | R: offline streaming/thinking tested; strict guarantee and plan eligibility unresolved |
| 20 | `zai-coding-cn` | Missing at baseline; now four pinned API-key models | K: independent endpoint/key tested offline; approved-client eligibility and strict guarantee unresolved |
| 21 | `mistral` | Native adapter and 34 pinned models; focused offline checks pass | Full host/family and live qualification remain |
| 22 | `minimax` | Missing at baseline; now M3/Flash API-key preset | K: complete replay/adaptive controls implemented; M2.7 availability and full qualification pending |
| 23 | `minimax-cn` | Missing at baseline; now M3/Flash API-key preset | K: independent key/current endpoint; M2.7 availability and full qualification pending |
| 24 | `moonshotai` | Six pinned models selected | K: offline batch verified; full provider qualification pending |
| 25 | `moonshotai-cn` | Six pinned models selected | K: independent regional routes verified offline; full qualification pending |
| 26 | `huggingface` | 87 pinned records bundled; native/debug/release host and full local gates pass | K: router/API/token defaults qualified offline; full provider qualification pending |
| 27 | `fireworks` | Missing at baseline; now all 27 pinned models | K: native/debug/release host batch and full local gates pass; full qualification pending |
| 28 | `together` | Bundled: 16 pinned models; native/debug/release host and full local gates pass | K: hybrid toggle versus GPT-OSS efforts, unknown cache pricing, full qualification |
| 29 | `baseten` | Fifteen pinned models selected | K: native/debug/release host batch verified; full qualification pending |
| 30 | `opencode` | Missing | K/S: Zen catalog and protocol mapping |
| 31 | `opencode-go` | Missing | K/S: separate plan catalog and routing |
| 32 | `kimi-coding` | Six pinned models selected | K: coding endpoint and dialect verified offline; full qualification pending |
| 33 | `meta` | Missing at baseline; now seven pinned API-key models | K/A: offline native batch passes; Muse subscription login and full qualification pending |
| 34 | `cloudflare-workers-ai` | Missing | C/S: token/account, chat and applicable classifier handling |
| 35 | `cloudflare-ai-gateway` | Missing | C/S: token/account/gateway and upstream auth separation |
| 36 | `qwen-token-plan` | Missing at baseline; now 20 pinned models | K: native/debug/release host and full local gates pass; full qualification pending |
| 37 | `qwen-token-plan-cn` | Missing at baseline; now 20 pinned models | K: regional keys, endpoint and full local batch pass; full qualification pending |
| 38 | `qwen-token-plan-individual` | Missing at baseline; now 13 pinned models | K: plan-specific primary key, regional fallback and full local batch pass; full qualification pending |
| 39 | `qwen-token-plan-individual-cn` | Missing at baseline; now 13 pinned models | K: independent China key/fallback and full local batch pass; full qualification pending |
| 40 | `xiaomi` | Three pinned PAYG models selected | K: full endpoint/model qualification |
| 41 | `xiaomi-token-plan-cn` | Two pinned models selected | K: full China plan qualification |
| 42 | `xiaomi-token-plan-ams` | Two pinned models selected | K: full Amsterdam plan qualification |
| 43 | `xiaomi-token-plan-sgp` | Two pinned models selected | K: full Singapore plan qualification |

Preserve these additional IDs and their saved-session/config compatibility:

| Local ID | Required preservation |
|---|---|
| `ollama` | No-auth local endpoint, `OLLAMA_HOST`, discovery and `/api/show` metadata |
| `decisions` | Existing OpenRouter classifier and helper/decider selection behavior |

For every reference row, the machine-readable manifest must include: upstream revision/source, supported operations, protocol per model, endpoint construction, auth methods and variable aliases, setup fields, discovery strategy, capabilities, metadata provenance, fixture cases and live receipt status. Missing implementation, missing credentials and unsupported capability are different states.

## 4. Implementation order

Effort: M = approximately day-scale; L = multi-day work, sometimes substantially longer. Estimates include tests but not waiting for accounts or upstream approval. They are not delivery promises.

| Phase | Deliverable | Dependencies | Effort | Change risk | Status |
|---|---|---|---|---|---|
| 00 | Pinned parity manifest and baseline gates | None | M | Low | IN PROGRESS |
| 01 | Correct registry, model overrides and readiness contract | 00 | M–L | Medium | IN PROGRESS |
| 02 | Maintained catalog and bounded discovery | 01 | L | Medium | IN PROGRESS: bounded discovery, tier pricing, explicit imports and generic image-history qualification implemented; other mappings and classifier selection pending |
| 03 | Broad compatible API-key provider batches | 01, 02 | L | Medium | IN PROGRESS: Groq/Cerebras/NVIDIA selected models, four xAI, seven Meta, four regional MiniMax, nine Xiaomi PAYG/token-plan entries, two DeepSeek, eleven ZAI global/China, eighteen Moonshot/Kimi, fifteen Baseten, twenty-seven Fireworks and sixty-six Qwen-plan models; remaining models, providers and complete qualification pending |
| 04 | Provider auth lifecycle and Magi/headless UX | 01, 02 | L | High | IN PROGRESS: stored API keys, configured browser PKCE and `:login` UI; provider-specific OAuth, device/cloud flows and complete qualification pending |
| 05 | Azure and Vertex completion | 01, 02, 04 | L | High | TODO |
| 06 | Bedrock transport and Mistral native dialect | 01, 04 | L | High | IN PROGRESS: Mistral adapter and 34-model offline foundation; host/live qualification and Bedrock remain |
| 07 | Gateway/classifier completion and existing-provider qualification | 02–06 as applicable | L | Medium–high | TODO |
| 08 | Full family, UI, packaging and live acceptance | 00–07 | L | Medium | TODO |

### Google tool-call fidelity foundation (2026-11-21, focused verification)

The shared Gemini/Vertex adapter now preserves distinct supplied IDs and generates
fallback IDs for missing, empty, invalid or duplicate ones. Request-local counters reserve
historical call/result IDs, so pure adapters need no clock or randomness. Matching call
and result IDs are serialized for the modern families selected by the
[pinned PI Google helper](https://github.com/earendil-works/pi/blob/5a7479a88390d947335a1a6609936897be0a7891/packages/ai/src/api/google-shared.ts).
Older model families keep their prior omission. The
[GenerateContent reference](https://aistudio.google.com/api-reference/generate-content)
defines the paired fields; this is not an Interactions API migration.

Four adapter failures first reproduced the identity defects. A native case then exposed
`[null, [], {}]` becoming an object in streamed arguments. Extracting arguments through
the existing exact-value decoder preserves nulls, container shapes and unsigned integers
without changing optional/control-field decoding. Five adapter tests and one native test
pass. The native case makes 24 loopback requests across both APIs, legacy/modern models,
supplied/generated IDs and fresh processes, verifying pairing and immutable history.

`oslo make test-provider-google-calls`, `test-streaming` (188 cases), `test-usage`,
Clippy and all seven native architecture gates pass. Evidence:
`../melchior/target/provider-foundations/google-calls-before-fix.log`,
`google-calls-native.log` (argument failure), `google-calls-native-exact.log` and
`google-calls-final-focused.log` in that directory. Full current-tree verification and
actual Magi-host qualification were not established by that call-ID-only checkpoint.
The subsequent signed-replay foundation below extends it; no new records are activated.

### Google signed replay foundation (2026-11-21, verified offline checkpoint)

Reinspection found an existing complete-turn replay contract for Responses and Anthropic.
Google now uses that same opaque journal carrier, preserving ordered signed response parts
without adding vendor-specific Magi events or clients. This replaces the earlier assumption
that a new neutral stream variant was required. Every signature stays on its original part,
including empty text/thinking and signature-only carriers; nothing combines signed parts.
This follows the [Google thought-signature guidance](https://docs.cloud.google.com/gemini-enterprise-agent-platform/models/thinking/thought-signatures).

Replay binds provider, model, API, resolved endpoint and credential/header lineage and
checks the visible text/thinking/tool projection. Unbound/imported raw signatures and
changed-origin state are not exported; visible history remains available. The local bounds
are 8,192 parts and 1 MiB of encoded state, with error rather than truncation on overflow.
Malformed/unsupported parts and unsuccessful finishes cannot create a replay envelope;
length stops remain non-executable, and late usage does not duplicate terminals.

Seven adapter regressions failed before implementation and now pass. Prior call-ID tests,
native 24-request regression, usage checks, Clippy and seven native architecture gates
also pass. Actual Magi-host execution passes both Google APIs in eight scenarios with
30 requests and eight Echo executions. It checks exact part replay after the tool round,
JSON/CBOR file serialization and restored `Session`s, empty signed messages, account/model
switches, partial-response retries with paid usage, and length/safety non-execution.
This is not a full CLI/Balthasar process-restart or cloud-authentication proof.

Evidence: `../melchior/target/provider-foundations/google-replay-before-fix.log`,
`google-replay-native-rounded-2.log` in that directory, and
`target/provider-foundations/google-replay-host-2.log`. The first full native run passed
1,012 library / 157 readiness tests but failed an obsolete image-corpus call-ID assertion.
The fixture now expects the exact generated ID; rerun `google-replay-verify-2.log` passes after
`test-fake-provider`. A further host regression reproduced signatures counting
as zero pending bytes; the fix counts text, thinking and tool-call signature carriers
and passes all three pending-memory regressions. The host/lint checks pass
in `google-replay-host-rounded.log`. Frozen evidence:
`target/provider-foundations/google-replay-20261121T135526Z-Ln8cUpTs/`.
Full native verification passes (1,012 library / 157 readiness tests in default/all-feature
runs and all later gates); sequential full Magi verification also passes. Copied debug
and release native images pass the Google host fixture and all previous host batches,
using the extracted public provider/catalog sources and an unchanged retained host image.
This source snapshot omits thought-token accounting; the subsequent focused fix described
above is outside its full-native claim. Discrete reasoning controls, broader model
qualification and complete durable/UI acceptance remain open.

The subsequent reasoning-control tranche resolves these pinned source inconsistencies:
Google's `gemini-4-hydrogen` and `gemini-4-neon` records declare `xhigh`, but the pinned
shared adapter rejects resolved levels outside minimal/low/medium/high. Gemma 4 mappings
use uppercase wire names. Aliases and robotics records need explicit control-family
review, not a guessed default budget. The new resolver uses reviewed native controls;
all 53 Google-API records now pass conversion and request-control checks. Real
[Hydrogen/Neon levels](https://docs.cloud.google.com/gemini-enterprise-agent-platform/models/guides/gemini-4-neon)
take precedence over the pinned helper's unsupported-level bug. This is still a control
foundation, not activation or complete remote protocol qualification.

Remaining Google catalog qualification has two verified upstream distinctions:

- The pinned Google catalog labels both Deep Research agents `google-generative-ai`,
  but [Google's agent documentation](https://aistudio.google.com/docs/deep-research)
  requires the Interactions API and background execution. Keep both identities in scope;
  do not activate them as ordinary GenerateContent models merely because their data shapes
  convert. Their actual lifecycle, cancellation, polling/stream recovery and accounting
  need an explicit implementation and qualification decision.
- Gemini 4 can emit a continuation boundary for long output. The pinned PI adapter
  deliberately disables automatic continuation and maps that boundary to `length`, with
  no hidden follow-up request. Qualify this same bounded contract before activation;
  automatic paid continuation is not required by the pin and must not be silently added.
  See the [pinned adapter](https://github.com/earendil-works/pi/blob/5a7479a88390d947335a1a6609936897be0a7891/packages/ai/src/api/google-generative-ai.ts)
  and [native continuation behavior](https://docs.cloud.google.com/gemini-enterprise-agent-platform/models/guides/gemini-4-neon).

### Ant Ling activation (2026-11-21, verified offline checkpoint)

The shipped declaration selects `Ling-2.6-1T`, `Ling-2.6-flash` and `Ring-2.6-1T`
with independent `ANT_LING_API_KEY` authentication at `https://api.ant-ling.com/v1`.
These are the exact three [pinned PI provider](https://github.com/earendil-works/pi/blob/5a7479a88390d947335a1a6609936897be0a7891/packages/ai/src/providers/ant-ling.ts)
records. All are text-only; Ring alone advertises reasoning at `high` and `xhigh`.
The [pinned Chat adapter](https://github.com/earendil-works/pi/blob/5a7479a88390d947335a1a6609936897be0a7891/packages/ai/src/providers/openai-completions.ts)
uses `reasoning.effort` only for explicitly mapped levels. The typed `ant-ling` dialect
preserves that distinction from generic effort support: absent, null, empty, unspecified
and nonreasoning controls are omitted. Explicit capability menus can restrict mappings
but cannot invent one. Missing `max` is not advertised; top-level `reasoning_effort`
remains absent even when Ring sends `xhigh`. No global default is changed.

Fourteen focused tests pass: four adapter, two compiler and eight native. Native
coverage includes 63 loopback requests: 24 efforts/defaults, 18 history/replay, six
tool changes, six auth/malformed failures, six retry requests and three cancellations.
Nine invalid/missing-auth requests and one unknown model refuse before HTTP. Required
strict schemas, null/empty/unsigned-max JSON, both reasoning fields, parallel tool
calls, image fallback and unknown-cache input-rate accounting are checked. Initial
failures demonstrated the missing dialect; fixture fixes preserve private `Options`
fields and the existing consumed false-only long-cache metadata policy.

Magi-host debug execution passes three models / six scenarios / 18 requests / 12 actual
Echo executions, including exact cards and Ring's `xhigh` serialization. Evidence:
`../melchior/target/provider-foundations/ant-ling-activation-rounded.log` and
`target/provider-foundations/ant-ling-host-debug.log`. Retained debug/release host runs
each pass this batch and earlier host regressions. Full Melchior verification passes
(1,000 library / 156 readiness tests in default/all-feature runs, plus later gates).
Full Magi verification passes on a sequential rerun after its shared-runtime hermetic
check detected concurrent Melchior tests in the first run. No gate logic or shared
runtime files were changed. Both results are retained in
`target/provider-foundations/ant-ling-activation-20261121T131826Z-xxVlQzx3/`.
The complete-parity gate correctly fails at 58 generic passes / 393 pending / zero live
requests. Subsequent Google work is outside this source checkpoint.
This is offline evidence, not remote strict enforcement,
full UI/durable restart, live access or complete provider qualification.

### Hugging Face activation (2026-11-21, verified offline checkpoint)

The shipped declaration selects all 87 pinned Chat records using `HF_TOKEN` and
`https://router.huggingface.co/v1`. Exact IDs, 66 reasoning flags, 35 image-capable
models, nullable effort maps and unknown rates remain unchanged. Six previously pending
`xhigh` records now convert. The compiler supplies the pin's implicit effort support,
`store=false` and `max_completion_tokens` only for the reviewed provider/API/base-URL
triple; explicit per-model overrides win. Global runtime defaults are unchanged.

Three compiler tests pass, including 696 in-process effort/default requests. Native
fixtures exercise all 87 shipped models, exact schemas and required strict tools,
both reasoning field names, parallel tool arguments, image adaptation, later-user
replay, tool changes, pricing, auth/malformed errors, retry identity and cancellation.
Initial failures were fixture expectations, not runtime-policy changes: selections are
sets rather than source order; unknown prices remain null rather than becoming free;
retry-success assertions allow the one retry they require. Failure logs are retained.
The final focused rerun and actual Magi-host fixture pass:
`../melchior/target/provider-foundations/huggingface-activation-rounded-2.log` and
`target/provider-foundations/huggingface-host-debug.log`.

Native totals are 696 effort requests, 522 history requests, 174 tool-change requests,
174 auth/malformed-error requests, six retry requests and 87 cancellations. Missing-token,
invalid-session and invalid-schema cases refuse 261 times before HTTP; an unknown model
also refuses. Actual-host debug and release batches each pass 87 models / 174 scenarios /
522 requests / 348 actual Echo executions, including all 35 vision models. Both retained
images also pass Together, Qwen, Fireworks, Moonshot/Kimi, Baseten and generic regressions.
Full Magi verification and full Melchior verification pass; Melchior includes 996 library /
148 readiness tests in both default/all-feature profiles, hermetic cleanup and later gates.
Sources and selected images match their retained snapshots after verification. Receipt:
`target/provider-foundations/huggingface-activation-20261121T125618Z-p4EzwCBH/`.

The actual-host fixture reads a hash-pinned reference and selected public provider source
at runtime, keeping Magi independently buildable. It exercises all 87 models and both
reasoning field spellings, without adding vendor clients to Magi. This does not prove
full UI behavior, durable CLI restart, confinement or remote inference.

The [pinned PI Chat implementation](https://github.com/earendil-works/pi/blob/5a7479a88390d947335a1a6609936897be0a7891/packages/ai/src/providers/openai-completions.ts)
and [Hugging Face adapter](https://github.com/earendil-works/pi/blob/5a7479a88390d947335a1a6609936897be0a7891/packages/ai/src/providers/huggingface.ts)
define this compatibility snapshot. Current [Hugging Face Chat documentation](https://huggingface.co/docs/inference-providers/en/tasks/chat-completion)
lists `max_tokens`; remote acceptance of the pin's implicit `max_completion_tokens`
requires separate qualification. No synthetic provider suffixes are added to model IDs,
and unspecified effort still follows NERV omission semantics. A strict-capability flag
does not establish that every routed backend remotely enforces a JSON schema.

### Together activation (2026-11-21, verified offline checkpoint)

The declaration selects all 16 pinned records, preserving the exact `TOGETHER_API_KEY`
identity, `/v1` endpoint, model IDs, limits, prices and nullable effort maps. Fifteen
records use `thinking_format="together"`; GPT-OSS retains `openai`. Typed hybrid control
sends `reasoning.enabled` and only model-supported `reasoning_effort`. No copied SDK or
Magi vendor client is introduced. Unspecified/null controls remain absent; this does not
claim equivalence with PI's default-disabled helper. The pinned records remain unchanged
when current [Together documentation](https://docs.together.ai/docs/inference/chat/reasoning)
describes different model defaults or effort aliases.

Fourteen focused tests pass. Native coverage is 128 default/effort requests, 96 streamed
parallel-tool/image/history requests, 32 dynamic-tool updates, 32 auth/malformed errors,
six retries across hybrid/effort/binary families and 16 cancellation requests. All 16
models replay both `reasoning` and `reasoning_content` without renaming the field. Native
cancellation waits for usage, kills/reaps the owned ask and observes transport closure;
the bounded fixture is shared with Qwen, whose 66 cancellation cases remain required.
Missing credentials, required strict guarantees and invalid sessions yield 48 pre-auth
refusals; an unknown model also dispatches no request. Ordinary/preferred tools do not
gain an unsupported strict flag, while JSON-schema answers retain exact JSON values.

Initial tests reproduced the absent dialect. Two fixture pricing assumptions were then
corrected: pinned zero cache rates mean unknown and use the existing input-rate fallback,
not free tokens; whole-micro estimates use explicit half-away-from-zero rounding. The
runtime accounting policy was not changed. Failure logs are retained beside
`../melchior/target/provider-foundations/together-activation-verify.log`.

Actual Magi-host debug execution passes all 16 models for both reasoning fields:
32 scenarios / 96 requests / 64 real Echo executions, with image adaptation for nine
vision models. Evidence: `target/provider-foundations/together-host-debug-verify.log`.
Full Magi verification and full Melchior verification pass (996 library / 140 readiness
tests in both profiles, followed by all later gates). Retained debug and release images
pass Together and Qwen/Fireworks/Moonshot/Baseten/generic host regressions. No live
inference was performed. The frozen source predates Hugging Face defaults work.

### Qwen activation (2026-11-21, frozen offline checkpoint)

Four declarations select all 66 pinned records, preserving exact regional URLs, primary
and fallback key order, offline bundled discovery, prices and per-model controls. The
same source IDs can occur in different plans without collapsing their provider identities.
Team and individual plans intentionally share a regional fallback variable, not the
plan-specific primary variable. No login or real configuration is changed.

One reviewed default was missing from the initial import fixture: PI's `detectCompat`
selects `max_completion_tokens` for Qwen. The shipped presets now declare that field
explicitly, and native/host tests require it while forbidding `max_tokens`. Current
[Qwen API documentation](https://docs.qwencloud.com/api-reference/chat/openai-chat)
also distinguishes the total-output limit from the deprecated model-dependent field.
The original foundation's `max_tokens` fixture is not proof of this corrected contract.

Fifteen focused tests pass: four adapter, two compiler and nine native. Native coverage:
540 original effort/key-precedence requests, 198 streamed tool/image/history requests,
132 dynamic-tool requests, 132 auth/malformed-stream requests, 24 retry requests across
three control families in each plan, and 66 process-cancellation requests. In addition,
198 missing-credential/invalid-session/required-schema refusals and four unknown models
dispatch no HTTP. Cancellation waits for partial usage, kills and reaps the owned ask,
observes transport closure and forbids a success/retry event; it is not full UI cancellation
acceptance. Retry fixtures run in isolated per-plan threads without changing backoff policy.

Actual Magi-host testing passes all 66 models, with 198 requests / 132 real Echo calls,
image handling for all 36 vision models, tool argument fidelity and later-user reasoning.
The first host attempt correctly found no Qwen cards before activation. A retry-fixture
compile error borrowed temporary JSON values; string comparison fixes the fixture
without changing production policy or assertions. Both failures are retained.

Full Magi verification passes. The prior Melchior foundation verification passed 992
library / 126 readiness tests in default/all-feature runs before these activation changes.
Final activation verification subsequently passes 992 library / 132 readiness tests in
both profiles and all later gates. Retained debug/release host proofs pass, and the
1,159-file artifact manifest verifies. This checkpoint is retained at
`target/provider-foundations/qwen-activation-20261121T120542Z-hyan2iJZ/` and predates Together.

### Qwen plan foundations (2026-11-21, before activation)

The four pinned variants contain 20 team/global, 20 team/China, 13 individual/global
and 13 individual/China records. The original native fixtures imported them explicitly
without adding shipped declarations. They preserve source IDs, regional endpoints, limits, modalities,
nullable controls and unknown token-plan prices; zero rates are not advertised as free.

Qwen's `enable_thinking` toggle and `reasoning_effort` are separate controls. The existing
adapter omitted the latter even for models declaring support. Three new regressions
failed before the fix. Requests now carry supported mapped effort values, and the native
control filter admits explicitly declared `xhigh` only with effort support. This resolves
12 incomplete candidate shapes. No source records or global compatibility defaults change.

Nine focused tests cover all 66 imported records at eight effort/default selections,
strict tools and structured-answer JSON fidelity, streamed cache/input/output accounting,
offline refresh, missing keys, regional isolation and primary/fallback precedence.
The native cases execute 528 effort requests plus 12 credential-precedence requests;
66 missing-key asks refuse without network. The two compiler tests separately exercise
528 in-process conversions, not HTTP requests. An initial test compile error used private
`Options` fields; the fixture now mutates `Options::default()` without changing visibility.

Pinned sources: [team/global](https://github.com/earendil-works/pi/blob/5a7479a88390d947335a1a6609936897be0a7891/packages/ai/src/providers/qwen-token-plan.ts),
[team/China](https://github.com/earendil-works/pi/blob/5a7479a88390d947335a1a6609936897be0a7891/packages/ai/src/providers/qwen-token-plan-cn.ts),
[individual/global](https://github.com/earendil-works/pi/blob/5a7479a88390d947335a1a6609936897be0a7891/packages/ai/src/providers/qwen-token-plan-individual.ts),
[individual/China](https://github.com/earendil-works/pi/blob/5a7479a88390d947335a1a6609936897be0a7891/packages/ai/src/providers/qwen-token-plan-individual-cn.ts)
and [Chat conversion](https://github.com/earendil-works/pi/blob/5a7479a88390d947335a1a6609936897be0a7891/packages/ai/src/api/openai-completions.ts).
Copies/digests are retained in `../melchior/target/provider-foundations/qwen-primary/`.

**Default boundary:** NERV's unspecified level still omits Qwen controls and lets the
server decide; PI's pinned Chat helper emits `enable_thinking=false` without an effort.
This fix does not claim exact default equivalence or silently disable always-thinking
models. Default/remote behavior remains a provider-qualification item. Current
[Qwen thinking documentation](https://docs.qwencloud.com/developer-guides/text-generation/thinking)
also describes model-specific controls; the pinned catalog is not rewritten to current
marketing aliases. Bundling, per-model image/history/error/retry/cancellation tests,
actual host proof, complete local verification and final qualification remain required.

### Baseten API-key batch (2026-11-21, frozen offline checkpoint)

One declaration imports all 15 pinned Chat records using `BASETEN_API_KEY` and the
exact `https://inference.baseten.co/v1` endpoint. Refresh is bundled/offline; no region,
gateway, account or model alias is inferred. Model URLs, prices, limits and explicit
null effort mappings remain intact. Kimi K3.1's runtime-only empty effort menu prevents
invented controls; the reference remains unchanged.

Both GLM 5.2 records use typed `chat_template_args.enable_thinking`: a literal boolean
or the exact `thinking.enabled` variable with optional `omitWhenOff`. Unknown keys,
variables, arrays and null toggles are refused rather than discarded or evaluated.
An explicit empty argument object clears inherited arguments. The adapter distinguishes
neutral `off` from the mapped `none` string, preserves unsupported-level omission, and
keeps the internal toggle out of outgoing requests. Two additional shapes become complete.

The pinned snapshot is the acceptance baseline, not a promise of current remote defaults.
[Baseten's current reasoning documentation](https://docs.baseten.co/inference/model-apis/reasoning.md)
describes different controls for GLM 5.2 Fast and Kimi K3.1 and notes that `none` can still
produce reasoning on GPT OSS. We retain pinned controls and document this drift instead
of silently changing the source or claiming remote conformance. GLM 5.3's pinned off-null
remains unsupported. [Session affinity documentation](https://docs.baseten.co/inference/model-apis/pricing-and-limits.md#session-affinity)
calls for a stable opaque conversation identifier; requests use the previously verified
scoped header mechanism, never the raw host transcript ID.

[Function-calling guidance](https://docs.baseten.co/inference/function-calling.md)
shows Chat function-level strictness and requires a result for every returned call ID.
[Structured-output guidance](https://docs.baseten.co/inference/structured-outputs.md)
documents JSON-schema answers and handling incomplete/refused results. These support
the selected request shapes; local fixtures do not prove remote schema enforcement.

Focused evidence: `../melchior/target/provider-foundations/baseten-native-qualified.log`
passes three adapter, two compiler and eight native tests. **276 local HTTP requests**
cover all eight effort/default choices on all 15 models, supported images, all strictness
policies, JSON-schema value fidelity, parallel tool/reasoning replay, later-user history,
dynamic declarations/removal, cache-token accounting, errors and retries. **75 pre-auth
refusals** and one unknown-model refusal dispatch nothing. All 30 catalog tests, exact
generation and seven unit/three native import tests pass. Actual Magi-host execution
against debug and retained-release Melchior passes all **15 models / 45 requests / 30 Echo executions**,
with stable captured affinity headers through tool rounds and later-user replay. The
18-model Moonshot/Kimi batch and existing 14-scenario regression also pass against that
image. Both repositories' `oslo make verify` pass; Melchior includes 980 library and
115 readiness tests in default/all-feature runs and the later hermetic/family/role gates.
Magi retains nine existing editor/visual skips. The explicit complete-parity gate correctly
exits 1 with **42 generic smoke passes / 409 pending / zero live requests**. Recipes:
`oslo make test-provider-baseten` in both checkouts; Magi requires `MELCHIOR_TEST_BINARY`
and `MELCHIOR_PROVIDER_SOURCE`. Retained evidence, images and extracted/hash-checked
source archives: `target/provider-foundations/baseten-20261121T103958Z-2erq7QOK/`.
Archives were not independently rebuilt; images are dynamic, not static-package proof.
Live entitlement, provider-specific cancellation, current vendor drift and final
qualification remain open, as do full-family/UI/production-restart requirements.

Initial failures are retained. Serde's derived struct accepted an empty sequence and its
optional field accepted null; typed map decoding now rejects both. Two native fixture
expectations were corrected: model cards use `max_output`, and OpenAI strict schemas
accept `pattern`, so the unsupported-schema fixture uses `additionalProperties:true`.
No production rejection or assertion was relaxed to make these fixtures pass.

### Session/cache identity foundation (2026-11-21, frozen local checkpoint)

The additive `Wants.session_id` field and `Capabilities.session_identity` flag remain
independently defined in Magi and Melchior. Legacy/nonadvertising model roles receive no
new request field. Magi hashes the project path, memory socket namespace and current
durable transcript; shared run IDs do not collapse distinct agent transcripts. The
current scribe wins over stale backend metadata after resume. Helpers omit identity.

Melchior validates 1–256 UTF-8 bytes without controls before authentication, strips the
raw ID before Lua conversion, and derives a 64-character key scoped to provider,
protocol, model, resolved endpoint and credential/header lineage. Chat-only compatibility
flags enable `prompt_cache_key` and/or one of three bounded, sensitive header dialects.
Moonshot/Kimi enable the body key. Identity remains stable across retries and OAuth
token rotation; separate sessions, models, accounts and endpoints do not share it.

**Important source default:** pinned PI `detectCompat`/`getCompat` chooses the OpenRouter
header dialect for its provider/URL, otherwise OpenAI, when an explicit format is absent.
The compiler now materializes that default for all 463 enabled Chat records (397
OpenRouter, 66 OpenAI). Native configuration still defaults to `generic`; runtime URL
guessing is not added. This resolves 443 incomplete shapes, bringing the candidate
count to **1,080/1,779**, with zero automatic activation. The 31 Anthropic affinity flags
and 38 Responses format flags remain pending.

**Verified focused evidence:** Melchior's `oslo make test-provider-session` passes
15 tests (six client/adapter, three compiler, six native), including **21 inference
requests, two OAuth refreshes and 11 refusals** in isolated local fixtures. Imported
OpenRouter/Baseten/Fireworks records are exercised through actual header capture.
All 28 catalog tests, exact regeneration, seven importer units and three native importer
tests pass. Magi's matching recipe passes four identity units and four integration
tests, including legacy role negotiation and nonempty history reconstructed in two
fresh host test processes. That disk-backed test fixture is not a production journal
or full CLI/Balthasar restart acceptance.

**Native host evidence:** all 18 Moonshot/Kimi models pass 54 requests and 36 Echo
executions against retained debug and release Melchior. The host now reads the selected binary's
actual cards instead of forcing session support in the fixture. Cache keys remain
identical across tool rounds and later-user prompts. The existing 14-scenario host
regression (28 requests / eight Echo executions) and four strict / 15 core / one
truncation tests also pass. Evidence: `target/provider-foundations/session-20261121T100747Z-PfARstSj/logs/host-debug.log`.

**Full Melchior verification passes:** 977 library and 107 native readiness tests in
default/all-feature runs, with the remaining hermetic, family/role, lint, documentation
and dependency gates passing. Log: `../melchior/target/provider-foundations/session-dialect-qualified.log`.
**Full Magi verification passes:** `target/session-wire-qualified.log`, terminal exit 0,
with nine existing editor/visual skips. The retained release host run also passes; its
confirmed log and explicit exit are in the frozen receipt below. The complete-parity
audit correctly exits 1 with **40 generic smoke passes / 411 pending / zero live requests**.
`target/provider-foundations/session-20261121T100747Z-PfARstSj/` now retains the frozen
pre-Baseten source, native images, catalog, logs and hash-checked artifact manifest.
Debug Melchior digest: `6da2b50b26be620aae29f4a793789716578940d84407e3c086166825baa5a3b3`;
release: `2c585f84888d7a6fd37b225707140dfc1c8baa4d6e0a25875ab5e217488f55c4`.
Archives were extracted/hash-checked, not independently rebuilt; binaries are dynamic.

Initial failures are retained: the new nested OAuth fixture needed an explicit module
path; the importer refusal fixture's Qwen record had become legitimately complete;
the role-negotiation fixture omitted then misnamed its output limit; and an exact model
refresh expectation omitted the new capability. These tests were corrected, not the
production refusals. The earlier `session-verify-fixed.log` run was deliberately stopped
after the pinned dialect correction made its source snapshot obsolete; it is not a pass.
Magi's `session-verified-limits.log` passed the full test suite and Clippy, then failed
the wire gate because that scanner treated SHA-256 length input as a network frame.
The gate now distinguishes an explicit digest-update statement; a new six-case regression
checks that uncapped reads/writes, including writes adjacent to the digest, still fail.
The corrected focused inventory is four identity units and four integration tests;
the new regression and fresh full gate pass.
Retention policy, other protocol affinity, production memory/CLI restart, remote cache
performance and complete provider/account qualification remain open.

### Moonshot/Kimi API-key batch (2026-11-21)

Three declarations import six pinned models each, without changing the reference or generated records. Both Moonshot regions use `MOONSHOT_API_KEY`; Kimi Coding uses `KIMI_API_KEY`. Their URLs and IDs remain exact, bundled refresh is offline, retries do not switch regions, and the User-Agent identifies `melchior`. Imported model URLs take priority over provider defaults, so an intentional regional override must update both levels.

The [Moonshot Chat reference](https://platform.kimi.ai/docs/api/chat) distinguishes optional K2.6 reasoning, forced/preserved K2.7 reasoning and K3 effort-only controls. K2.6's runtime-only `moonshot_keep_thinking` flag sends `keep="all"` unless thinking is disabled. Binary menus expose off/high or high-only; K3-style models retain low/high/max and explicit nulls. JSON-schema answers use the documented request format; required strict function tools remain unsupported according to the pinned records.

[Kimi model guidance](https://www.kimi.com/code/docs/en/kimi-code/models.html) warns about model fallback for wrong IDs and disabled thinking. Unknown local IDs are refused before dispatch; forced-thinking models never send off. [Service guidance](https://www.kimi.com/code/docs/en/) distinguishes China and overseas endpoints and requires truthful client identity. API-key access does not qualify subscription OAuth, account limits or model entitlement. Stable session cache-key wiring, called for by the Chat reference for Coding Plan sessions, remains open.

**Verification:** `oslo make test-provider-moonshot` in Melchior checks an exact 11-test inventory: two adapter, one compiler, eight native tests. It makes 264 isolated loopback requests, with 36 pre-auth refusals and three unknown-ID refusals. All 18 models cover eight effort/default choices, images, usage, preferred tool and answer-schema payloads, parallel tools, later-user reasoning, dynamic declarations/removal, auth errors, malformed streams and retries. Magi's recipe of the same name uses `MELCHIOR_TEST_BINARY` and `MELCHIOR_PROVIDER_SOURCE`; all 18 models pass actual host tool execution (54 requests / 36 Echo executions per run) against debug, selected release and retained-copy Melchior. The retained release also passes the existing 14-scenario / 28-request host regression, four strict-preflight tests, 15 core tests and one truncation test. Both full local verification suites pass; Melchior also passes 25 catalog tests/exact generation and import tests. Magi retains nine explicit editor/visual skips. Native images are dynamically linked, not static-package acceptance. Logs and source/image evidence are under `target/provider-foundations/moonshot-20261121T093425Z-1B6Vx0nm/`; no live inference or login occurred.

The explicit complete-parity gate exits **1**, correctly reporting **40 generic smoke passes / 411 pending / zero live requests**, with report `../melchior/target/provider-parity/run-1795253843404715613-2580434/`. Dedicated native suites do not silently promote the broader pending cases.

**Next dependency:** stable session identity is absent from the host-to-model request; the broker currently leaves `Ask.about` empty and it must not be repurposed as a per-session key. Add a bounded, optional neutral session/cache identity with durable resume and replacement-role compatibility, then map provider opt-ins. This supports Coding Plan cache keys and the pinned affinity fields still blocking Baseten/Fireworks records. Other provider/model batches and full qualification remain in scope.

### Dynamic tool-history foundation (2026-11-21)

**Contract:** both independent model definitions now accept one `Content::ToolChanges { added, removed }` block per System message. With history present, `Context.tools` is the initial state. Replay removes before adding, replaces definitions without changing their order, and retains caller data unchanged. Chat requires both positional-System and tool-addition flags to emit inline declarations. A removal or any repeated name folds through the last non-additive change; subsequent first-time additions remain anchored. Other adapters receive the final tool set and their existing instruction fallback. This follows [PI's pinned transcript semantics](https://github.com/earendil-works/pi/blob/5a7479a88390d947335a1a6609936897be0a7891/packages/ai/src/utils/transcript.ts).

Inline Chat declarations use complete System `tools` messages, separate from text, as required by the [Kimi dynamic-tool contract](https://platform.kimi.ai/docs/guide/use-dynamic-tool-loading). Limits are 4,096 active tools, 16,384 declarations/removals, 8 MiB of declarations and 1–256-byte control-free names. Invalid histories refuse before credential refresh. Strict normalization includes inline tools, and the endpoint hook receives only the combined tool projection. Magi's required-strict preflight now replays current policies, preventing inline declarations from bypassing replacement-role capability confirmation without rejecting tools already removed or replaced.

**Focused proof:** the exact-inventory Melchior recipe passes 13 tool-history adapter tests, six System regressions, ten strict regressions, two compiler tests and five native-process tests. The vector oracle covers all 4,096 four-change sequences in both modes (8,192 replays). All nine registered adapters serialize fallback identically to an explicit current set; this does not turn Decisions into a tool-execution API. Native cases cover twelve requests, nine pre-auth refusals, retries, later-process replay, strict-only inline routing, lossless old calls and immutable input. Initial test-helper visibility, example CBOR dependency and strict-normalizer lint failures are retained alongside repaired logs.

The actual Magi broker proof caught a serialization defect missed by the first focused suite: inline schemas had not entered Lua's typed JSON boundary, so empty arrays changed shape, null fields disappeared and large unsigned integers lost precision. `Content.added[].parameters` now uses the same exact-value conversion as top-level tool parameters. Engine, adapter and native HTTP regressions cover it; the original host assertion remains unchanged. A separate existing capability-test script hit `ETXTBSY` during full Magi verification. Its fixture now uses the existing bounded executable-readiness helper before measuring the production probe; no runtime capability behavior or assertion was weakened.

**Catalog:** all eighteen pinned `supportsMidConvoToolAdditions` flags are mapped; fourteen records become complete shapes. Four Fireworks records still require affinity support. All 1,779 scoped records remain present, 637 are complete candidate shapes, and 21 declarations / 41 selected imports are unchanged. The generated catalog is not automatic activation.

**Final verification:** `../melchior/target/provider-foundations/tool-history-verify-lossless.log` passes full verification: 969 library and 93 native readiness tests in both feature runs, all catalog tests, hermetic/family/model-role gates, Clippy and dependency/security checks. `target/tool-history-verify-fixture-ready.log` passes full Magi verification with nine existing editor/visual skips. The focused event-collection repair passes all 141 tests. Both selected and retained-copy Melchior pass the expanded Magi proof (14 scenarios, 28 HTTP requests, eight Echo executions); the same recipe passes four strict-preflight, 15 core and one host truncation test. Replacement-role cases cover required inline tools, removals, replacement and re-addition without mutating the ask.

Receipt: `target/provider-foundations/tool-history-20261121T084823Z-bczIjUhS/`, retaining source, pre/post source hashes, native images, repaired/failing logs and the negative parity report `../melchior/target/provider-parity/run-1795251505587907998-2309591/`. The complete-parity gate correctly exits 1 with 34 generic smoke passes / 417 pending / zero live requests. The receipt's binaries are dynamically linked, not static-release proof; copied source has not been independently rebuilt. Production/durable generation and role negotiation for System/tool-change entries, named instruction sections, Anthropic native tool changes, provider-specific model/account availability and full family/UI/static/live acceptance remain open. Next: qualify Moonshot global/China and Kimi Coding without treating API-key access as subscription OAuth support.

### ZAI regional streaming and reasoning batch (2026-11-21)

**Implemented:** the existing `zai` preset selects seven pinned models; new `zai-coding-cn` selects four. Their Coding Plan endpoints and keys remain distinct, and discovery is bundled/offline. The old global `glm-4.6` model is replaced without an alias; choose a current ID or declare a custom model. All eleven raw records remain unchanged. The compiler maps `zaiToolStream` to a boolean Chat capability and keeps true flags on unsupported protocols pending. Complete candidate shapes increase from 612 to 623, not full qualification.

**Native behavior:** only requests with actual tools send `tool_stream=true`. Reasoning models using the ZAI dialect explicitly preserve historical reasoning with `clear_thinking=false`, including when no effort is selected. GLM-5.2's `off -> none` sends native effort `none`; GLM-5.3's null off mapping never sends disabled. Binary models expose off/high, not fake distinct depths. Supported image inputs, tool-result images, exact indexed arguments, later-user history, cache accounting and unknown highspeed pricing are tested. See the [global control contract](https://docs.z.ai/guides/capabilities/thinking.md), [preserved-thinking requirements](https://docs.z.ai/guides/capabilities/thinking-mode.md) and [China streaming contract](https://docs.bigmodel.cn/cn/guide/capabilities/stream-tool).

**Unresolved guarantees:** PI claims strict tools for these models, but the reviewed [Chat schema](https://docs.z.ai/api-reference/llm/chat-completion.md) does not establish that guarantee. The explicit runtime overlay disables strict guarantees pending proof; raw/generated strict=true remains intact. Preferred tools keep their original schema, while required tools and unsupported JSON-schema answers refuse before credential access. Imports replace nested compatibility objects, so each reviewed override restates all pinned fields; an initial native failure caught the missing tool-stream flag. This discrepancy remains required work, not a removed parity requirement.

**Eligibility:** the [China Coding Plan rules](https://docs.bigmodel.cn/cn/coding-plan/tool/others) restrict use to named clients; NERV is absent from the inspected list. Establish permitted use before live requests. No client impersonation, login or account inference was performed. Global/CN model availability and Coding Plan entitlement are not proved by fixtures.

**Evidence:** `../melchior/target/provider-foundations/zai-inventory-fixed.log` passes three adapter, two compiler and six native tests. The exact-inventory recipe guards against empty or changed filters. The six native tests cover all eleven models through 129 loopback requests: 88 effort/no-tools cases, 33 parallel-tool/follow-up/later-user requests, four auth/malformed-stream cases and four retry attempts. Offline listing/key isolation and invalid-before-credential cases dispatch nothing. `zai-catalog.log` passes all 22 compiler/catalog tests and exact regeneration; `zai-verify.log` passes full Melchior verification, including 956 library and 88 readiness tests in each feature configuration.

**Host and remaining gates:** `target/zai-magi-verify.log` passes full Magi verification with nine existing skips. `target/zai-host.log` passes ten actual-host scenarios / 20 local requests / eight Echo executions, plus 15 core-turn tests and the host truncation regression. Two scenarios exercise ZAI reasoning across parallel tools, retry and a later user turn; other scenarios preserve prior Chat/System coverage. The same recipe passes using the retained Melchior copy. Complete parity still exits 1 with 34 generic passes / 417 pending (`../melchior/target/provider-parity/run-1795249222756565866-2097158/`), after 50 native preset and seven shared tests pass. Native suites are not yet aggregated into the other parity requirements. Receipt: `target/provider-foundations/zai-20261121T081422Z-wE11nkx6/`; source archives are captured after verification, not independently rebuilt. Selected Melchior is `70ad0a4781732133462bb76e6728e2dfe72ca6823ebe01446414ed083e0c83c4`; Magi remains `d8271772123447d304755c0be2232d70d92fb9ac65307a1ea4c5ef75a7126495`. Both are dynamic binaries. Remote strict support, service eligibility and whole-family/UI/restart/static/live gates remain unproved.

### DeepSeek pinned-model batch (2026-11-21)

**Implemented:** the existing `deepseek` provider now explicitly imports `deepseek-flash` and `deepseek-v4-pro` from the pinned snapshot. Discovery is offline, and only `DEEPSEEK_API_KEY` enables the preset. The old bundled `deepseek-chat` / `deepseek-reasoner` model entries are replaced, not silently aliased; saved selections must choose a current ID or use an explicit custom declaration. The provider ID remains unchanged. Pinned limits, prices, nullable effort mappings and Flash resize policy are preserved. Reviewed overlays enable native effort selection, refuse unsupported JSON-schema answers and choose the DeepSeek strict-tool dialect.

**Strict contract:** `chat_strict_dialect = "deepseek"` is explicit configuration, not inference from a provider name or reasoning format. Every tool must consent and normalize successfully before any strict set is sent. A mixed set falls back intact only when strictness is preferred; a required tool makes an incompatible mixed set fail before credentials. No constraints are dropped or ordinary tools silently upgraded. The portable normalizer currently refuses references, structured unions, optional/null schemas and unsupported constraints; this is a documented strict-schema coverage limit, not full acceptance of DeepSeek's complete native schema vocabulary.

**Routing and isolation:** the new pure adapter `request_endpoint` hook receives only prepared tool metadata. The Lua protocol selects beta only for an effectively strict request, replacing a terminal `/v1` and preserving a proxy prefix. Ordinary requests keep their configured path. A query-bearing strict base fails rather than being concatenated incorrectly. Existing origin validation still runs before credential access, including refresh; redirects are not introduced. No new vendor transport or Magi client is added.

**Evidence:** `../melchior/target/provider-foundations/deepseek-roundtrip-fixed.log` passes eight schema tests, ten strict-adapter tests and seven native tests. The native tests make 32 local requests: 16 effort selections, six streamed-tool/follow-up/later-user turns, eight effective strict-route cases and two retry attempts. They also check offline key-isolated listings and zero-request invalid-schema/mixed-policy/origin/query/answer-format refusals. Flash user and tool-result images are preserved; Pro omits images with the explicit text placeholder. Both models retain tool and non-tool reasoning read from actual process events. Pro keeps System updates in position while Flash uses the leading-prompt fallback. Terminal usage, cache reads and pinned estimates are checked after intermediate null usage. Catalog tests pass all 20 cases plus exact regeneration (`deepseek-catalog.log`).

**Full gates:** `deepseek-verify-fixed.log` passes the full local Melchior gate, including 953 library tests and 82 readiness tests in both default/all-feature runs, integration/examples, Clippy/rustdoc, architecture/hermetic/interface/role and dependency checks. The earlier `deepseek-verify.log` failed only its stale shipped-model assertion (952 passes / one failure). Initial fixture mistakes remain in the other `deepseek-*.log` files. The complete parity recipe intentionally still exits 1 with 32 generic passes / 419 pending / zero live requests; its new report is `../melchior/target/provider-parity/run-1795247317946071524-1921019/`. Its prerequisite preset suite passes 44 native tests and seven shared tests.

**Consumer and receipt:** Magi's full `instructions-magi-verify.log` passes the current Magi code, retaining nine editor/visual skips. `deepseek-host.log` repeats eight actual Magi-host/selected-Melchior scenarios (13 local requests, four Echo executions); the same recipe passes again using the retained Melchior copy. The receipt is `target/provider-foundations/deepseek-system-20261121T074927Z-9EcUwgUt/`, with source archives, digests, binaries and logs captured after verification. Archives are not independently rebuilt. Melchior SHA-256 is `b189bb7057b07e2082567fada3e15cbfc9a3b036734d56024a967ab8a99e9536`; Magi is `d8271772123447d304755c0be2232d70d92fb9ac65307a1ea4c5ef75a7126495`. Both are dynamically linked. No live provider entitlement, remote strict grammar acceptance, full selected-family/UI/restart or static release is claimed.

### Ordered System-message foundation (2026-11-21)

**Implemented:** `Role::System` and `Message::system` exist in both independent model definitions. `supports_mid_convo_system_messages` inherits field-by-field, including an explicit false override. Chat keeps opted-in instruction messages in order and uses the declared system/developer role. Otherwise, nonempty System text is folded into `Context.system` in sequence with blank-line separators; the caller's history remains unchanged. Non-text/signed blocks and assistant-only metadata are rejected before credential refresh or inference. A true positional flag on another adapter fails explicitly when System history is present instead of silently moving it.

**Reference and boundary:** this follows PI's [pinned plain-text transcript fallback](https://github.com/earendil-works/pi/blob/5a7479a88390d947335a1a6609936897be0a7891/packages/ai/src/utils/transcript.ts). Named instruction-section patching, dynamic tool-set deltas and Magi production/durable System-entry creation remain separate work, not claims of this foundation. The compiler maps 42 Chat flags and leaves 73 other-API flags pending; complete candidate shapes rise from 602 to 612 with **zero new activations**.

**Focused proof:** `oslo make test-provider-instructions` passes six adapter, two compiler and three native-process tests (`../melchior/target/provider-foundations/instructions-all-fixed.log`). The all-adapter case checks the exact nine registered names and verifies instruction text actually reaches each serialized request, including Decisions. Bedrock remains an unimplemented tenth enum, not a skipped implemented adapter. Native cases cover 14 loopback requests, inheritance, Unicode, tool-history integrity, retry/later-turn placement and invalid-before-refresh behavior. Magi's `test-provider-completions` passes eight native scenarios, 13 HTTP requests, four actual Echo executions, 15 core tests and the host truncation regression (`target/provider-foundations/instructions-host.log`).

**Verification status:** the first full Melchior run failed with 947 passes because the new all-adapter test included unregistered Bedrock. The corrected run `instructions-verify-fixed.log` completed successfully with 948 library tests in each default/all-feature run; Magi `instructions-magi-verify.log` also passed, retaining nine explicit editor/visual skips. DeepSeek development overlapped later Melchior stages, so this is not a frozen-source DeepSeek acceptance receipt. The fresh DeepSeek run above is required for the newer catalog and tests.

**DeepSeek reference basis:** [tool documentation](https://api-docs.deepseek.com/guides/tool_calls/) specifies beta routing and whole-set strictness. [Thinking documentation](https://api-docs.deepseek.com/guides/thinking_mode/) requires retaining earlier reasoning when tools are present. The newer batch above implements these contracts without treating catalog shape completeness as proof of runtime behavior.

### Shared Chat indexed-tool checkpoint (2026-11-21)

**Implemented:** `../melchior/config/apis.lua` maintains bounded per-index function-call state. The first call streams incrementally; later calls buffer until the response terminal and emit in index order. Delayed ID/name fields and identical repeats work; conflicting identities, missing indices/metadata, malformed shapes and invalid final JSON refuse executable completion. Raw argument bytes preserve Unicode, nulls, empty containers and full-range integers. Indexless single calls and ID-addressed continuations work; anonymous interleaving is refused rather than guessed. Bounds are 256 calls, 4 KiB per ID/name and 1 MiB aggregate identity/argument bytes.

**Terminals and lifecycle:** `length` and Xiaomi `repetition_truncation` end with non-executable `length`; filtered/error/unknown reasons end with `error`. Valid `[DONE]` can close a legacy response, but EOF without a protocol terminal cannot. Late usage remains chargeable, duplicate terminal events do not emit another stop, and post-terminal content cannot create new calls. Retry resets both the active call and buffered calls; cancellation closes transport and cannot contaminate a later stream. Final cumulative usage includes abandoned attempts.

**Reference basis:** the [OpenAI Chat event reference](https://developers.openai.com/api/reference/resources/chat/subresources/completions/streaming-events) identifies tool-call indices and finish reasons. PI's [pinned Chat implementation](https://github.com/earendil-works/pi/blob/5a7479a88390d947335a1a6609936897be0a7891/packages/ai/src/api/openai-completions.ts) associates calls by index/ID and fills delayed metadata. NERV serializes its indexed state into the unchanged sequential wire instead of introducing a cross-program dependency.

**Focused evidence:** Melchior `oslo make test-provider-completions` passes 12 new adapter cases, ten existing adapter cases, four HTTP transport cases and four actual-process cases. These cover byte-at-a-time delivery, interleaving, delayed/repeated identity, typed malformed input, exact capacity boundaries, retries, cancellation, terminals, late usage, tool-result replay and a later user turn. The preset recipe also passes 40 native and seven shared tests. Logs are under `../melchior/target/provider-foundations/chat-indexed-*.log`.

**Actual consumer evidence:** Magi's recipe of the same name requires an explicit `MELCHIOR_TEST_BINARY`. Its isolated executable fixture runs the real host turn loop against that binary and loopback HTTP: success and retry each execute two correctly assembled Echo calls, retain exact in-memory transcript arguments/results, and replay them on the tool round and a later user turn. Four length/repetition/filter/unknown-terminal scenarios execute nothing. Six scenarios total 11 requests and four tool executions. The recipe also passes 15 core turn tests and the host truncated-call test; Magi Clippy and architectural gates pass. Selected Melchior SHA-256 is `be99c70b70a57630128abcb8efbc0ea9475fd69557f37c8af0eca5406d15f4d2`; the host fixture SHA-256 is `31e9bcc577194f4846424c11dd89cc673fc8fc5eb7603f3f9e9f01f70e0358a9`. These are dynamically linked artifacts. The fixture uses no scribe: it does **not** prove durable journal/resume, the complete selected family or UI behavior.

**Retained regressions and fixture repairs:** initial tests reproduced argument cross-association and the incorrect repetition terminal. Additional tests exposed malformed JSON returning nil without throwing, and `choices: [false]` bypassing structural validation. Both now refuse executable completion. The native retry assertion initially read the first rather than final cumulative `spent` event. Full runs exposed 10-second fixture deadlines overlapping the unchanged 10-second production retry base; only the new Chat and prior Xiaomi test deadlines were increased to 30 seconds. Host fixture assertions were aligned with existing contracts: a subsequent provider round drains pending entries without a scribe, and `error` stops do not commit tool entries, whereas `length` preserves failed attempted calls. Production accounting, retry policy and host execution rules were not weakened.

**Verification status:** fresh complete Melchior and Magi `verify` runs pass, including their hermetic, architectural, interface, documentation and dependency gates. Melchior passes 942 library tests and 72 native readiness tests in each default/all-feature run. Magi retains nine existing explicit editor/visual skips; its per-program family-interface gate is not complete selected-family acceptance. Catalog checks pass 18 tests and exact regeneration against all 43 pinned source files. The fresh complete-parity recipe intentionally fails with 32 generic passes and 419 pending cases, zero live inference; receipt `../melchior/target/provider-parity/run-1795245263226625807-1694321/`. Full logs, retained regressions, public references, source archives and selected binaries are retained at `target/provider-foundations/chat-indexed-20261121T070448Z-ajYO6XZg/`. Archives are captured after verification and digest-checked, not independently rebuilt. No declaration, selected import, catalog marker or remote entitlement changed. Custom/grammar calls, provider-specific reasoning/usage dialects, the 25 undeclared integrations and full provider/auth/cloud/host-restart/UI/static/live qualification remain open.

### Xiaomi PAYG and regional token-plan checkpoint (2026-11-21)

**Implemented:** `xiaomi` selects Flash, Pro and Pro-UltraSpeed from the pinned V2.6 catalog; `xiaomi-token-plan-cn`, `xiaomi-token-plan-ams` and `xiaomi-token-plan-sgp` each select Flash and Pro. Each declaration has its own key variable, bundled discovery and regional endpoint. All nine pinned source records remain unchanged. Full target coverage is 20/45 declarations, with 28 selected imports; it is not a qualification percentage.

**Protocol:** the current [Chat reference](https://mimo.mi.com/static/docs/api/chat/openai-api-post.md) accepts Bearer authentication, `max_completion_tokens` and strict function tools. [Deep-thinking requirements](https://mimo.mi.com/static/docs/quick-start/usage-guide/text-generation/deep-thinking.md) require complete reasoning replay on tool-call history, including subsequent user turns. The adapter preserves visible reasoning and fills missing assistant `reasoning_content` with an empty string only on declared reasoning models. Explicit false overrides work; other roles and nonreasoning models are unchanged. Existing alternate reasoning fields and inline thinking retain their encoding; opaque signatures never become request keys. Off/high map to disabled/enabled, not separate depths.

**Independent capabilities:** the [answer-format docs](https://mimo.mi.com/static/docs/quick-start/usage-guide/text-generation/structured-output.md) promise JSON-object output, not schema enforcement. `supports_json_schema = false` therefore refuses structured-answer schemas before credential acquisition or dispatch, without disabling strict function tools. No silent degradation to prompt-only JSON is introduced. Undeclared adapters retain their existing behavior; this does not newly qualify their schema support.

**Accounts and cost:** [regional setup](https://mimo.mi.com/static/docs/tokenplan/Token%20Plan/quick-access.md) and [key guidance](https://mimo.mi.com/static/docs/quick-start/faq/api-integration.md) distinguish PAYG from token-plan endpoints and credentials. Local readiness is not an entitlement check. Token-plan zero rates remain unknown, not free; PAYG rates remain pinned estimates rather than current invoices. Keys are never shared across the four declarations.

**Local evidence:** `../melchior/target/provider-foundations/xiaomi-presets-rounded.log` passes **40 native tests plus seven shared adapter tests**. Seven Xiaomi tests exercise **142 loopback requests**: 72 control selections, 27 tool/later-user turns, 27 output caps, eight typed failures and eight retry requests. Offline key-isolated listing and schema/image/missing-key refusals do not dispatch. `xiaomi-catalog-presence.log` passes **18 catalog tests** plus deterministic regeneration. `xiaomi-verify.log` passes full Melchior verification: **926 library tests** in each default/all-feature run, integration/examples, Clippy, rustdoc, architecture/hermetic/interface/role and dependency gates. Its binary is dynamically linked, not a static release. Magi's `xiaomi-magi-refresh.log` passes **67 distinct tests** and `xiaomi-magi-gates.log` passes architecture gates. Source inputs remain immutable; the compiler resolves 50 reasoning-field markers and produces 602 complete shapes, without activating other newly complete candidates.

**Failures and incomplete acceptance:** initial logs retain the untyped large test literal, ceiling-versus-nearest-micro estimate expectations, and a cost-object fixture that ignored the explicit all-false price-presence mask. Only tests were corrected; production accounting was not weakened. The complete-parity gate deliberately still fails: **32 generic text/usage passes, 419 pending, zero live requests**, with receipt `../melchior/target/provider-parity/run-1795243018488278262-1482726/`. Passing native presets does not promote those broader pending rows. The Xiaomi source/log/primary-document snapshot is `target/provider-foundations/xiaomi-20261121T063712Z-boKp4vlJ/`; archives are not independently rebuilt, and selected-family/UI/static/live acceptance is not claimed.

**Shared Chat gap discovered at this checkpoint:** indexed argument fragments could attach to the most recently started call, and Xiaomi `repetition_truncation` fell through to `end_turn`. The later shared Chat checkpoint above implements bounded ordered assembly and non-executable interruption handling. Xiaomi's original single-call preset tests did not cover these defects; its frozen receipt remains historical, not proof of the later fix.

### MiniMax global/China checkpoint (2026-11-21)

**Implemented:** `minimax` and `minimax-cn` select M3 and M3.1 Flash each. Keys are region-specific, discovery is bundled/offline, and both provider and imported-model endpoints are explicitly scoped. Current [China documentation](https://platform.minimax.cn/docs/api-reference/text-anthropic-api.md) names `api.minimax.cn`, overriding the pinned `api.minimaxi.com` host without changing immutable source inputs. Both regional documentation pages list only M3 and Flash on the Anthropic interface. All eight pinned MiniMax records are retained; four M2.7 records remain unactivated and unresolved, not deleted from the parity target.

**Protocol:** the [global interface contract](https://platform.minimax.io/docs/api-reference/text-anthropic-api) requires complete assistant-content replay and automatic-only tool choice. Melchior adds opt-in bounded replay preserving ordered thinking/text/tool blocks, exact integers/null/containers and provider fields across the neutral transcript. It binds replay to credential/header lineage, provider, model and resolved endpoint, and checks the visible projection. Changing identity drops only opaque blocks, not visible tools. M3 advertises off/minimal (one enabled state); Flash advertises low/medium/high/xhigh/max, with unsupported off/minimal retained as null. Structured-answer schemas and required-strict tools refuse before credentials.

**Evidence:** `../melchior/target/provider-foundations/minimax-transport.log` passes 30 adapter/transport/native/compatibility/control tests, including single-byte chunks, retries, cancellation/socket closure, context immutability and bound-size refusal before expired OAuth refresh. `minimax-catalog-fixed.log` passes 16 catalog tests and deterministic regeneration. Compiler mappings are protocol/type-scoped; 587 candidate shapes are complete and 168 xhigh-adapter markers remain. No implicit activation follows from a complete shape. `minimax-responses-regression.log` passes the shared credential/JSON/Responses regressions. `minimax-presets-final.log` passes 33 native tests plus two shared tests; eight MiniMax tests exercise 72 loopback requests, including image/tool replay, pricing/tier boundaries, output caps and retries. `minimax-verify-fixture.log` passes full local verification: 921 library tests in each default/all-feature run, integration/example suites, Clippy, rustdoc, architecture/hermetic/interface/role/dependency gates. This builds a dynamic local binary, not a static-package or selected-family receipt. Magi's `minimax-magi-refresh.log` passes 67 distinct tests and `minimax-magi-gates.log` passes all architecture gates.

**Retained failures:** replay state initially lost large integers/null/empty containers in the legacy Lua scratch bridge; opt-in scratch now crosses that bridge as encoded JSON. New terminal tests reproduced replay publication on length-limited output and numeric-signature coercion; both are fixed and pass. Unknown/missing/error terminals and malformed streams cannot publish executable completion or replay state. Initial native/compiler test compile errors and a one-micro rounding error in the tier test fixture remain in `minimax-*.log`; no production rate was altered to satisfy the fixture. First full verification exposed an invalid generic Anthropic fixture with orphan text deltas; the fixture now supplies indexed block start/delta/stop events. The stricter adapter was not weakened. Failure log `minimax-verify.log` and receipt `../melchior/target/provider-parity/run-1795241011384601829-1304306/` are retained.

**Batch gates and retained source:** the explicit `test-provider-parity`, `test-streaming` (176 tests), `test-usage`, `test-overflow` and `test-cache-contract` recipes also pass. `test-provider-parity-complete` correctly fails with **24 generic text/usage passes, 427 pending cases and zero live requests**; negative receipt `../melchior/target/provider-parity/run-1795241635197889971-1373094/`. Snapshot `target/provider-foundations/minimax-20261121T061428Z-8CdUKiEa/` retains logs, source archives/manifests, base commits, dirty diffs/status and that negative report. It includes 695 Magi and 359 Melchior files, including all 43 immutable catalog JSONs; private checkout `.env.lua`/`.magi.lua` are excluded. Live/extracted source digests are checked. Archives have not been separately rebuilt, and no selected-family/UI/static/live receipt is claimed.

**Still open:** M2.7 service availability/routing, remote account claims, complete auth/model/provider qualification, selected-family host/UI/static/live acceptance and all other unfinished phases. The 45-integration and 1,779-record targets are unchanged. No live inference, login, installation or real configuration changes have been performed.

**Next-batch availability warning:** the [Ant Ling deprecation notice](https://developer.ant-ling.com/en/docs/models/deprecation/) lists all three pinned Ant Ling model IDs for retirement on 2026-11-15, before this checkpoint. Its heading still says upcoming, but the date has passed and the page was updated on November 19. Do not activate those IDs merely to increase the count. Retain their reference evidence and review a separately documented current-model overlay; the pinned `ant-ling` thinking dialect also differs from the current tutorial. Exact old-model availability remains unverified without an authorized service request.

### Meta API-key checkpoint (2026-11-21)

**Implemented:** the `meta` preset selects all seven pinned Muse Ember/Spark/Wonder chat records over Responses with `META_API_KEY` and bundled discovery. Other providers' keys and `MODEL_API_KEY` are not accepted aliases. Native tests cover all seven models' readiness, limits, effort mappings, cached/tiered estimates, encrypted reasoning/tool continuation, image input/results, model-switch isolation, typed errors and retries. The selected model never changes to a Contributor variant on retry. See `../melchior/catalog/activations.md` for the complete model list and request counts.

**Display labels:** Meta's [Contributor terms](https://dev.meta.ai/docs/muse-spark) permit training on submitted content. The two Contributor variants carry `training-enabled` labels. An initial regression showed that `Card.name` is deliberately a bare routable ID, used by Magi's short-name resolver. The fix adds optional `display_name` to both model-role twins and Magi choices, preserving `id`/`name` routing. CLI listings, the highlighted picker caption and ACP option labels present it without sending it as a model ID. The picker strips control characters and bounds captions; ACP validates labels within its existing catalog bounds. This is disclosure, not an account-consent or service-entitlement claim. `store=false` does not override data-use terms.

**Evidence:** `../melchior/target/provider-foundations/meta-presets-fixed.log` records **25 native preset tests plus two shared adapter tests passing**, with **109 Meta loopback requests** across seven tests. Fresh Melchior `oslo make verify` passes **910 library tests** in both default/all-feature runs, integrations/examples, Clippy, rustdoc, architecture/hermetic/interface/role/dependency gates (`meta-verify.log`). The complete parity gate fails as required with **20 generic text/usage passes, 431 pending and zero live calls**; negative receipt `../melchior/target/provider-parity/run-1795238272487468392-1053490/`. This count does not promote the separate native preset tests to complete provider rows. Magi's final label recipe passes **54 distinct tests** (`meta-labels-final.log`; three nested child results are duplicate executions), and all-targets Clippy passes (`meta-magi-clippy-fixed.log`). Broader `test-model-refresh` passes 67 distinct tests (one overlapping filter and nested child results are not additional cases), and final architecture gates pass. Logs: `meta-magi-refresh.log` and `meta-magi-gates-final.log`.

**Retained failures:** the initial label assertion and missing-field compilation failure remain in `meta-presets.log` and `meta-presets-label-fixed.log`. Magi's first label build lacked a test import; its first architecture pass exceeded the existing 4,000-line protocol budget. New consumer-level wire checks now live in the host, and the old card fixture uses equivalent legacy JSON; no gate limit or assertion was removed. `meta-magi-gates-fixed.log` passes all architecture gates including twins and protocol size. Clippy also caught picker enum growth: captions now share the existing boxed presentation state rather than increasing the overlay's inline size. Final label tests and Clippy cover that correction. The receipt contains source archives with live/extracted digest checks and all 43 raw catalog inputs; archives have not been separately rebuilt, and no new selected-family binary receipt is claimed.

**Still open at this historical checkpoint:** Muse subscription OAuth, provider-specific account/cancellation and full-family/UI/static/live acceptance. MiniMax was disabled then; its subsequent implementation is recorded above. All other unfinished phases and 1,779 reference records remain in scope.

**MiniMax follow-up from that checkpoint:** the adaptive-control, complete-replay and regional fixtures described above implement the next batch. Resolve the remaining M2.7 availability drift explicitly, preserve all eight source records, and finish broader acceptance before calling either region fully qualified.

### xAI API-key checkpoint (2026-11-21)

**Implemented:** the shipped `xai` declaration selects `grok-4.3`, `grok-4.5`, `grok-4.6` and `grok-4.7` from the immutable PI 1.6.0 catalog. It uses `XAI_API_KEY`, `https://api.x.ai/v1/responses` and bundled-only discovery. All four pinned chat records are selected; this is not complete xAI support because the reference's subscription authentication and remaining acceptance cases are still open. See `../melchior/catalog/activations.md` for primary-source review and model-specific details.

- **Model contract:** native cards preserve context/output ceilings, image resize metadata, prices and supported efforts. Grok 4.3 sends `none` for neutral `off`; 4.5–4.7 do not advertise disabling reasoning. Grok 4.5's `xhigh` selection maps to `high` as an explicit alias. Required-strict tools refuse before authentication; preferred tools omit strict, matching the pinned compatibility default. Unsupported long cache-retention options remain absent.
- **Continuation:** requests use `store=false`; exact encrypted reasoning and output items survive split tool arguments and the next request's user/tool images. Existing endpoint/model/credential binding remains in force. No wire field or Magi production code changed in this batch.
- **Cost:** Responses now handles `usage.cost_in_usd_ticks` using exact unsigned-integer JSON and decimal quotient/remainder conversion to native microdollars. Present ticks override dollar `cost`; malformed ticks produce one anomaly and an unknown measurement. Missing ticks preserve existing behavior. Half-microdollar rounding is deliberate; raw tick precision is not retained. The legacy tally's zero-charge estimate policy is unchanged, while measured accounting distinguishes zero from unknown.
- **Settlement:** pinned prices and cached-input tier thresholds are tested for each model, including 200,000 versus 200,001 total prompt tokens. Reported positive costs replace estimates. Completed, failed and incomplete terminal events charge once. A billed response followed by a truncated HTTP body and a successful retry retains both charges; the next request is blocked at the shared cap without HTTP.

**Verified:** four new adapter tests cover all three Responses hostings, malformed types, absent/zero values and rounding boundaries above 2^53 through `u64::MAX`. The preset recipe passes two shared adapter tests plus 18 native test functions, ten specific to xAI (**79 xAI loopback requests**). The parity recipe adds three passing inventory/harness tests; its report remains **18 generic text/usage passes, 433 pending, zero live requests**. `test-provider-parity-complete` was also run and fails on those pending cases as required; that failure is retained, not relabeled as success.

| Gate | Result for this checkpoint |
| --- | --- |
| Melchior `oslo make verify` | PASS: 910 library tests in each default/all-feature run, integration/example suites, Clippy, rustdoc, architecture/hermetic/interface/role/dependency gates |
| Melchior `oslo make test-provider-catalog` | PASS: 14 catalog tests and deterministic `provider-catalog --check` |
| Melchior `oslo make test-provider-responses` | PASS: 88 distinct selected tests; log also contains six nested OAuth test-process results |
| Magi `oslo make test-model-refresh` | PASS: 68 tests; zero-test filtered binaries are not counted as test passes |
| Root `oslo make test-provider-images-family` | PASS: eight adapters, 40 HTTP requests, 168 mutation refusals; receipt `../target/acceptance/run-lLalMnGx/`, 103.639 seconds for the selected scenario |
| Complete provider parity | FAIL as expected: 433 pending cases; no full-provider completion claim |

**Source-packaging repair:** a global `**/data` Git ignore rule hid all 43 immutable reference JSON inputs. Narrow exceptions in `../melchior/.gitignore` now retain only the pinned catalog directory/files; Git enumeration includes all 43. Earlier Git-enumerated source snapshots omitted these inputs, so use this checkpoint's archive for native source reproduction. The raw input digests and generated catalog are unchanged. The source snapshot excludes per-checkout `.env.lua`/`.magi.lua` and decorative `misc/` assets, not build/test/catalog inputs.

**Receipts:** `target/provider-foundations/xai-20261121T044559Z-DNWTHlJa/` retains native source snapshots/manifests, binaries, logs, positive/negative parity reports and selected-family evidence. Working logs are in `../melchior/target/provider-foundations/xai-*.log` and `target/provider-foundations/xai-{magi-refresh,family}.log`. The initial regression log retains the four ignored-tick failures and the incomplete test-helper compilation failure. The first fresh parity report is `../melchior/target/provider-parity/run-1795236221598345676-807976/`; the expected complete-gate failure is `../melchior/target/provider-parity/run-1795236837865586839-884748/`. Native images remain dynamically linked; static packaging is not established.

**Still open:** xAI Grok/X subscription OAuth, provider-specific cancellation/handoff and family/UI/static/live proof; all remaining provider/model/protocol batches, cloud credentials, and broader acceptance. No real credential values were inspected, no live inference/login performed, and no installation, commit or push occurred. The target remains 45 integrations and 1,779 pinned records.

### Implementation checkpoint — 2026-11-20

**Implemented:**

- `../melchior/tests/provider-parity/manifest.json` records all 43 reference provider IDs and the retained `ollama` and `decisions` integrations. The pinned registry and its MIT license are retained alongside it. Registry SHA-256: `1b184fc2a5099b2005154be51a3a81074dba739b8d83b58849835aff22c43952`.
- Each reference row records its source digest, protocols, operations, endpoint/auth metadata, required cases and separate live status. Model catalog and discovery metadata remain **pending review**; the manifest is not a completed certification.
- `test-provider-parity` executes credential-free, isolated HTTP fixtures through the shipped Lua adapters and Rust client. It validates text, usage, route and terminal-event behavior for the currently exercised protocols. `test-provider-parity-complete` fails while any required case is pending. No provider count alone satisfies either gate.
- Provider declarations now distinguish omitted model protocols from explicit overrides. Direct deserialization and Lua catalog assembly preserve mixed APIs; serialization round trips and explicit Chat Completions overrides are covered.
- Malformed provider declarations now produce provider/field-attributed, value-redacted diagnostics instead of disappearing from the catalog. Two discovery fixtures that depended on silently dropping malformed declarations now use valid empty declarations.
- Model cards now carry optional secret-free `setup` metadata, mirrored in both independent wire definitions. Legacy `ready`/`needs` and cards without the extension remain supported. States distinguish missing, expired or unreadable credentials, incomplete configuration, missing protocol implementations and unimplemented authentication. Variable requirements preserve alternative names within required groups.
- Melchior checks nonblank environment credentials, stored OAuth freshness/refreshability, endpoint admissibility and registered request/event handlers. Listing reads OAuth state once without token exchange. Fresh stored tokens can perform inference without refresh configuration; missing refresh configuration still prevents renewal.
- Magi re-queries the basic model role after environment changes instead of inferring readiness from one variable. Failed queries preserve model metadata but mark readiness unconfirmed. Catalog reads now bound elapsed time, response bytes and model count, and own/clean up the provider process group. The standalone model-list command uses the same reader. These Magi changes are still under verification below.
- Setup presentation belongs to `magi-host`, not the wire crate. The 4,000-line protocol gate and wire-twins checks are retained unchanged in strength; the latter now includes `Setup` and `SetupState`. `ROLES.md` documents the additive contract.
- Removed the old second, optional-refresh query after an environment update. It raced the authoritative basic query, produced stale duplicate notifications and rejected replacement model programs without `--refresh`. The real-host integration now passes all six readiness transitions with basic-role-only fixtures.
- Model declarations now support `base_url` and provider/model `headers`. Header precedence is protocol, provider, then model, case-insensitively; ambiguous duplicates, invalid values, authentication/signing/framing overrides and oversized sets are refused without echoing values. Provider-level `credential_origins` explicitly binds any additional exact origins. Final adapter URLs are checked before credential lookup or OAuth refresh; discovery/cache models cannot inject routes or headers. `../melchior/EXTENDING.md` documents this contract. Verification is recorded below rather than inferred from these declarations.

**Verified checkpoints:**

| Command / stage | Result | Evidence |
|---|---|---|
| Melchior `test-provider-foundations` | PASS: 15 endpoint, 24 catalog, 2 discovery integration tests | `/tmp/providers-foundations.log` |
| Melchior `test-provider-parity` initial harness | PASS: 3 tests; 10 contract cases passed, 441 pending, 0 live requests | `/tmp/providers-harness.log` |
| Melchior `test-provider-parity-complete` | EXPECTED FAIL: 441 required cases pending | `/tmp/providers-harness-complete.log` |
| Initial Magi `verify` | FAIL under former sandbox: proxy fixture `EPERM` | `/tmp/providers-baseline-magi.log` |
| Unrestricted Magi `verify` | Tests, hermetic gate and build progressed; FAIL at missing advisory database | `/tmp/providers-baseline-magi-unrestricted.log` |
| Initial Melchior `verify` | Tests, architecture/hermetic gates progressed; FAIL at missing advisory database | `/tmp/providers-baseline-melchior.log` |
| First family baseline | FAIL: user shell wrapper selected as Bash | `/tmp/providers-baseline-family.log` |
| Family baseline with explicit GNU Bash | FAIL: Bubblewrap absent from PATH | `/tmp/providers-baseline-family-bash.log` |
| Root `prepare-advisories` in Melchior's Nix development environment | PASS: reused workspace-private pinned advisory inputs | `/tmp/providers-prepare-advisories-ready.log` |
| Magi `audit` and `deny` with workspace advisory cache | PASS; dependency duplicate-version warnings remain nonfatal | `/tmp/providers-magi-audit-ready.log` |
| First post-foundation Melchior `verify` | Tests passed; FAIL at Clippy in new fixture's use of `last()` on a double-ended iterator; changed to `next_back()` | `/tmp/providers-foundations-verify.log` |
| Melchior readiness `test-provider-foundations`, `clippy`, `gates` | PASS: 45 focused tests, including four isolated real-binary readiness/inference cases; Clippy and all architecture gates pass | `/tmp/providers-readiness-melchior-final.log` |
| Magi readiness `gates` | PASS, including protocol size and wire twins | `/tmp/providers-readiness-magi-gates-final.log` |
| Magi readiness `test-provider-foundations`, `clippy` after duplicate-query repair | PASS: 11 wire, 4 bounded-reader, 20 catalog, 2 refresh and 2 real-host environment tests; Clippy passes | `/tmp/providers-readiness-magi-single-query.log` |
| Melchior usage, JSON and initial parity regression recipes | PASS; parity remains 10 cases passed / 441 pending / 0 live inference | `/tmp/providers-readiness-usage.log` |
| Melchior full `verify` after routing/readiness changes | PASS: all-target/default and all-feature tests, Clippy, rustdoc, architecture/wire, hermetic, built-binary family/model-role, dependency and advisory gates; built artifact is dynamic, not static-package acceptance | `/tmp/providers-routing-verify.log` |
| Magi async CLI repair: `check`, expanded `test-provider-foundations`, `clippy` | PASS, including CLI model unit tests and both real-process logging tests | `/tmp/providers-magi-async-listing.log` |
| Magi full `verify` after async CLI repair | PASS: tests, Clippy, architecture/wire, hermetic, build and dependency/advisory gates; dynamic binary, not static-package acceptance | `/tmp/providers-magi-async-verify.log` |
| Melchior initial bounded-discovery `test-provider-discovery`, `clippy` | PASS: 35 discovery unit tests and 5 real-process integration tests; Clippy passes | `/tmp/providers-discovery-tests-fixed.log` |
| Melchior discovery hardening `test-provider-discovery`, `clippy` | PASS: 43 discovery unit tests and 5 integration tests, including response limits, per-provider/overall deadlines, cancellation, atomic caches and Ollama metadata fallback; Clippy passes | `/tmp/providers-discovery-bounds.log` |
| Corrected root `test-family`, selected images `run-fKCSPbRT` | FAIL: selected Magi predates the async CLI repair and both `noting` tests panic on nested Tokio runtime; runner fixtures and four-image integrity checks pass | `/tmp/providers-baseline-family-ready.log` |
| First Melchior discovery `verify` | FAIL: new forty-provider test returned in 20.04 seconds but started all 40 queued requests at the deadline; explicit pre-poll/pre-job deadline checks added | `/tmp/providers-discovery-verify.log` |
| Discovery deadline repair `test-provider-discovery`, `clippy` | PASS: 43 unit and 6 real-process tests; forty stalled providers finish in 20.05 seconds and at most 16 requests start; Clippy passes | `/tmp/providers-discovery-deadline-fixed.log` |
| Melchior full `verify` after deadline repair | FAIL: four spending fixtures still seeded obsolete ID-only caches; no requested cached model was loaded | `/tmp/providers-discovery-verify-fixed.log` |
| Melchior `test-usage` and `test-overflow` after fixture repair | PASS: all 9 spending tests, 6 discovery integrations, 2 real-process overflow tests and the recipes' unit/regression suites; existing charge/admission assertions retained | `/tmp/providers-discovery-fixtures.log` |
| Root selected-family `test-cache-contract` | PASS: cache-contract scenario, all four current selected binaries and both fixture images checksum-verified; not a full-family run | `/tmp/providers-discovery-family-cache.log`, `../target/acceptance/run-julFDibt/` |
| Root `test-eval-bindings` after offline fixture/template updates | PASS: 64 frozen identity, mutation/refusal and exact effective-configuration checks; no workers started | `/tmp/providers-discovery-eval-bindings-ready.log`, `../target/evaluation/binding-tests-5fl8hpat/` |
| Melchior discovery `verify` after fixture repair | All default/all-feature tests, Clippy and rustdoc pass; FAIL at wire gate because the new special-file cache test used `std::fs::create_dir`; fixture changed to `places::ensure` | `/tmp/providers-discovery-verify-fixtures.log` |
| Melchior `test-provider-catalog` and `gates` | PASS: 2 offline input integrity/identity tests across all 43 source files; every architecture/wire gate passes without rule changes | `/tmp/providers-catalog-inputs.log` |
| Melchior full `verify` after discovery, fixture and catalog-audit repairs | PASS: default/all-feature all-target tests, Clippy, rustdoc, architecture/wire, hermetic, built-binary family/model-role and dependency/advisory gates | `/tmp/providers-discovery-catalog-verify.log` |
| Melchior full `verify` with context-tier pricing and unknown base-rate presence | PASS: default/all-feature all-target tests, Clippy, rustdoc, architecture/wire, hermetic, built-binary family/model-role and dependency/advisory gates | `../melchior/target/provider-foundations/run-20261120T212104Z-bWWjqW/verify.log` |
| Melchior `test-provider-catalog`, `clippy`, `gates` with native converter | PASS: 8 source/mapping/refusal tests, exact deterministic output comparison, Clippy and unchanged architecture/wire gates | `../melchior/target/provider-foundations/run-20261120T213301Z-j5no6U/catalog.log` |
| Melchior full `verify` after catalog converter | PASS: all default/all-feature targets and full local gates, including generated-output reproducibility | `../melchior/target/provider-foundations/run-20261120T213301Z-j5no6U/verify.log` |
| Root selected-family `test-cache-contract` with tier-pricing image | PASS: cache-contract scenario in 12.974 seconds, all four selected programs and both fixture images checksum-verified; not full-family acceptance | `../target/acceptance/run-17X8Hcdb/`, `../target/acceptance/provider-tier-cache-native.log` |

**Current verification boundary:** both Magi and Melchior full local `verify` commands pass at retained checkpoints. Melchior's latest pricing/converter checkpoint is `../melchior/target/provider-foundations/run-20261120T213301Z-j5no6U/`, including full/focused/family-cache logs, source archive/digests, tracked patch, result metadata and a checksum-verified read-only executable, SHA-256 `cba62a1ccc1aab25c3f8d48c4e09e4a0d06751398f3392b47648eb74d7cb898e`. This image and Magi `a1807b53cd75b46c02bc2a4676c2dd7b1e6252b55b027ecb890e307dad3e9d05` also pass the new selected-family cache receipt above. These root checks are scenario-only, not a new full-family or static-package run. Root frozen-evaluation bindings remain passing at their earlier checkpoint. No verification process from this checkpoint remains in flight.

The pricing-only checkpoint remains at `../melchior/target/provider-foundations/run-20261120T212104Z-bWWjqW/`, with the same runtime image hash; the converter is maintenance tooling, not a runtime activation. The first pricing verification lost its process handle and `/tmp` log during an environment reset; neither completion nor success was established, and it is not counted as a pass. Subsequent evidence is retained under `target/` rather than relying on temporary logs.

The new root cache check initially failed before any scenario: Balthasar's directory environment could not find Cargo (`run-EVfkVTaG`); an isolated retry inherited Oslo directory state that removed the explicit Bubblewrap path; another retry correctly refused a utility directory containing an unselected installed Magi (`run-YVYgtgye`). No gate was relaxed. The passing run used Melchior's Nix development toolchain, private `target/provider-build-env/{config,data}` XDG directories, `unset OSLO_DIRENV`, explicit GNU Bash/Bubblewrap, and Nix utility paths ahead of the global system profile. No user configuration, Balthasar source or installed binaries were changed. Preserve the failed receipts and `../target/acceptance/provider-tier-cache{,-isolated,-clean}.log` alongside the passing `-native.log`.

Melchior's passing discovery checkpoint is retained at `../melchior/target/provider-foundations/run-20261120T205516Z-h3GLEd/`: full verification, discovery, regression and catalog-audit logs; source digests; tracked patch; result metadata; and a read-only checksum-verified executable. Its image matches the selected-family cache receipt above. The build is dynamic; static/release acceptance remains pending.

The discovery deadline repair passes its focused suite and Clippy. Tokio timeout polls its inner future before reporting expiry; checks are explicit before starting queued jobs and polling another result. The original real-process test remains strict. Full verification then exposed old ID-only cache assumptions in four spending fixtures. They now prime v2 via the real isolated listing command before injecting versioned/legacy price metadata, preserving all accounting/admission checks. Spending, overflow and event-usage fixtures explicitly disable unrelated dynamic providers instead of seeding obsolete empty caches. The family acceptance/evaluation configurations and frozen render templates receive the same fixture-only update; production discovery is not disabled to make tests pass.

Magi's full passing checkpoint is retained at `target/provider-foundations/run-20261120T203706Z-mlkI64/`: full/focused logs, source digests, tracked patch, result metadata and read-only executable copy, SHA-256 `a1807b53cd75b46c02bc2a4676c2dd7b1e6252b55b027ecb890e307dad3e9d05`. This proves the repaired dirty tree, not a new selected-family or static-package run.

The corrected family run used GNU Bash, the existing Nix-store Bubblewrap executable and the workspace advisory cache. Its complete runner-fixture suite passed, including the 37 native event/auth fixture families' six invalid-receipt states. Its selected Magi image was built before the async CLI repair and the native suite failed in both real `magi models` logging tests on nested Tokio runtime creation. Model-list dispatch now awaits the bounded reader directly, startup diagnostics omit provider output and credential values, and the expanded focused recipe passes both real-process tests. This terminal family failure is attributable old-image evidence, not current-image proof or a pristine pre-change baseline. A new complete selected-image run remains required.

Melchior's earlier post-foundation `verify` passed Clippy, rustdoc, architecture and hermetic stages, then its build overlapped the next wire edit and failed on the temporarily missing `Card.setup` initializer (`/tmp/providers-foundations-verify-fixed.log`). That initializer is implemented and the fresh full `verify` above now passes. Magi's first readiness run needed its relocated CBOR test changed to use the existing IPC codec (`/tmp/providers-readiness-magi.log`); the next run exposed the duplicate-query bug (`/tmp/providers-readiness-magi-final.log`). Both repairs pass the focused recipe and Clippy in the table above. A mistaken `check-all` invocation was rejected because Magi names that recipe `check`; it provided no verification. Magi's audit/deny repair is verified separately, not represented as a successful uninterrupted full `verify` run.

Routing qualification in `/tmp/providers-routing-foundations-fixed.log` passes 15 endpoint, 4 routing, 25 catalog, 2 discovery and 6 isolated readiness/inference tests, plus 142 provider streaming/regression tests. Its Clippy stage caught a nonminimal readiness boolean; that expression was corrected and the subsequent uninterrupted full `verify` above passes. The first routing integration attempt failed because a fixture used an untyped empty Lua table for `credential_origins`; it now uses the established `melchior.json.array` constructor, without weakening declaration validation (`/tmp/providers-routing-foundations.log`).

Melchior's passing checkpoint is retained at `../melchior/target/provider-foundations/run-20261120T201515Z-zy6E8L/`: full and focused logs, source-file digests, result metadata and a checksum-verified read-only copy of the built executable. This proves this dirty-tree checkpoint, not a committed release, static build or full provider parity.

The selected family images and integrity reports are retained under `../target/family/run-fKCSPbRT/`. Preserve this failed receipt when rerunning the current source; it must not be relabeled as a pass after a separate focused repair.

**Phase 02 discovery checkpoint:** declarations now select bundled, compatible, OpenRouter or Ollama discovery. Compatible requests use an explicit auth header scheme and the same provider headers/origin restrictions as inference. Missing required keys skip networking; fresh stored OAuth access can be used without refresh/login, while cloud discovery remains unavailable. OpenRouter public and account-filtered `/models/user` listings are separate and only the public popularity-sorted response publishes rank. Network work is capped at four concurrent providers, five seconds per provider and twenty seconds overall. Results are bounded to 8 MiB/4,096 models per provider; malformed/duplicate/empty identities reject the listing. Failed queries retain declared or last-good models, successful empty listings replace them, and slow Ollama metadata does not discard a completed model listing.

Discovery cache v2 hashes provider configuration, endpoint, strategy and credential identity, never raw values in filenames. Private owned files and anchored directory traversal reject symlinks, hard links, special files and unsafe permissions; bounded writes use atomic replacement. Old ID-only caches are not migrated across an unverifiable account boundary. Tests prove key/alias/header/endpoint partitioning, stale fallback, byte/count caps, cancellation socket closure, FIFO refusal and concurrent-reader integrity. This is not an offline-catalog completion: normal loads still fetch absent/expired dynamic catalogs, cloud auth is not implemented, and the pinned catalog has not yet been shipped.

Parity runs retain their reports, worker output and executable/configuration digests under `../melchior/target/provider-parity/run-*`. Retention is verified by `run-1795203081468200569-1724039`: its runner metadata and case report agree on 45 integrations, 10 passed cases, 441 pending cases and zero live requests. Temporary baseline logs are diagnostic evidence, not long-term release receipts.

The complete gate's retained receipt is `run-1795203193173975210-1800210` (`/tmp/providers-harness-complete-receipts.log`). Its worker succeeds in producing the inventory; the parent gate correctly fails because 441 required cases are pending. A successful worker exit is not full-parity acceptance.

**Catalog-source discovery for Phase 02:** PI's Git tree omits generated `src/providers/data/*.json`. The published `@earendil-works/pi-ai@1.6.0` tarball includes them; it was downloaded without installing or executing the package and its npm SHA-512 integrity was verified. Tarball SHA-256: `d68312ec574bc959cb7fee26da4f4876d067aed9bbbe76e876359e60d2294747`. The metadata has no `gitHead`, so this is a separately identified release-data snapshot, not proof of byte identity with the source commit. It contains 1,698 chat, 81 classifier and 74 image-generation records across the same 43 provider IDs. Image-generation records remain excluded from runtime scope. All 43 unmodified input files are now retained under `../melchior/catalog/reference/pi-1.6.0/data/`, with PI's MIT notice and `../melchior/catalog/README.md` provenance/import constraints. Every file was rechecked against the parity manifest's SHA-256 before retention. These are review/generation inputs, not activated shipped models. A future generator must use these pinned local inputs rather than silently re-fetching moving models.dev data.

**Additional catalog review findings:** the 1,779 in-scope records include one chat/classifier identity collision: `openai/gpt-6-luna` appears in both Responses and Decisions. PI flattens catalogs separately by operation; NERV's current `provider/model` lookup cannot silently flatten both into one entry. Preserve chat selection and introduce an explicit classifier-operation identity/selection contract before importing both. Of these records, 303 carry context-price tiers and 48 carry model headers. There are 143 records with zero input and output rates, but only one explicitly carries `free: true`. Pinned `catalog/sources/models-dev.ts:213–228` converts absent prices to zero, so package zeros cannot establish free service or known cache rates. Preserve unknown-price provenance and apply context tiers before claiming accounting parity. The catalog also carries input limits, prompt-cache modes and 30 compatibility keys; mapping only names, windows and base rates is insufficient. These are discovered requirements, not completed implementations.

**Phase 02 pricing implementation:** native costs now support bounded, validated context tiers. The highest threshold strictly below the full prompt count selects rates for the whole attempt, including cache reads/writes; omitted tier rates inherit the base schedule. Each retry remains separately priced, and a final positive provider charge replaces only that attempt's estimate. Base input/output presence now distinguishes explicitly declared free rates from generated or legacy unknown zeros, as cached rates already did. Invalid/duplicate/unbounded schedules are rejected before request credentials or networking. Unit, Lua-loading, real spending and usage-event fixtures cover boundaries, cached context, retries, provider settlement, cap refusal and unknown-rate telemetry. Full verification now passes, including all 12 real spending tests and the usage-event integration. This does not implement separate one-hour cache-write accounting, activate reference model schedules or change the legacy admission policy for unknown prices. `../melchior/EXTENDING.md` documents the declaration and accounting behavior.

**Phase 02 catalog conversion checkpoint:** `../melchior/catalog/compiler/` and the native `provider-catalog` maintenance example deterministically convert the pinned local source into `../melchior/catalog/generated/pi-1.6.0.json`. The generated artifact is a candidate/report, not runtime declarations. It preserves all 1,779 chat/classifier identities, all 303 tier schedules, 48 model-header maps, per-provider source hashes and 74 explicitly excluded image identities. Unknown snapshot zeros are not inferred to be free. Unmapped fields are individually reported rather than silently dropped from a supposedly supported model; the source files retain their full original values. All 43 provider activation states remain unreviewed. Twenty model records have complete native data shapes, which does not qualify provider behavior. Mapping requirements include image constraints on 1,290 records, strict-mode compatibility on 980 and `xhigh` thinking keys on 869, including explicit unsupported levels. `oslo make catalog-generate` writes the artifact; `test-provider-catalog` verifies exact reproducibility plus source/mapping/identity/refusal tests. Neither command fetches metadata or executes PI/Node code. Runtime import, operation-aware selection, provider defaults/auth and qualification fixtures remain required.

**Next Phase 02 work:** finish the reported metadata semantics and connect reviewed generated catalogs to runtime declarations, including operation-qualified classifier selection, before enabling additional presets. `xhigh` requires both independent message types and effective controls, not just accepting another string. The initial baseline offered six fixed reasoning levels without deriving selectable controls from static model thinking maps; the reasoning checkpoint below addresses that gap. Strict/schema compatibility and image constraints are further high-coverage mapping workstreams. Preserve every reference record and report unsupported mappings explicitly; model conversion alone cannot certify provider auth, protocol defaults or runtime behavior. Normal dynamic-provider startup still fetches absent/expired catalogs, so offline bundled-startup acceptance remains open. Review the separate on-demand `describing.rs` HTTP path as well: it now uses the private cache helper, but its legacy `.json()` fetch, bearer-only request construction and base/key-only description identity have not been brought under the new bounded discovery/header configuration contract.

**Reasoning-control contract:** preserve `capabilities.thinking` as raw published metadata and add optional `capabilities.thinking_levels` for supported provider-neutral control names. An explicit empty list means no selectable controls; an absent list keeps legacy six-level behavior rather than inferring `xhigh`. Both independent message models must preserve the new level and optional field. Static maps and discovered effort restrictions must drive controls and request mapping consistently. Budget-only Anthropic/Google adapters cannot claim a native `xhigh` effort merely because a model map contains that string; their missing native effort/adaptive behavior remains explicit catalog work. No provider activation is implied by this additive wire change.

**Reasoning-control implementation and evidence:** both independent models now carry `xhigh` and optional neutral controls, with wire-twins coverage. Static maps and discovery restrictions drive Melchior's effective mappings; raw published metadata remains unchanged. Magi's picker, stepping and model card use supported controls, and the host refuses unsupported manual selections. `ModelChanged` now carries optional authoritative thinking state to every client; model switches synchronize session state and persistence with the configured default used by the replacement backend. This repairs stale attached-screen settings and a mismatch between the displayed reasoning level and the new worker. Older events without the field retain the client's current value.

Magi's `target/provider-foundations/thinking-current-fixed.log` records 2 model, 3 application, 6 picker, 12 model-card and 2 native-host tests passing; the existing print-only model-card example remains explicitly ignored. The native host test proves two attached clients, fresh snapshots, persisted choices, refusal without mutation, and actual requests before and after a model switch, in an isolated HOME/XDG process. Melchior's focused recipe passes 5 control, 8 adapter, 5 discovery and 7 real-process readiness tests; the new fixture checks JSON and CBOR cards and the HTTP request's `reasoning_effort`. Nine catalog tests prove deterministic conversion and preservation of unsupported mappings. These are offline fixture and rendered-string tests, not interactive terminal or live-provider acceptance.

Melchior's complete `oslo make verify` passes at `../melchior/target/provider-foundations/run-20261120T220641Z-ljoh01/`, including 842 library tests in each default/all-feature run, full integration suites, Clippy, rustdoc, architectural/hermetic/role/dependency gates and the built binary. Source archive/digests, tracked patch, focused/full logs and the executable are retained. Executable SHA-256: `73c6e9d81a2e7f6089bea4ce087abf1fc12da863185c189a044a078e34cbda59`. This is a dynamic native image, not static-package or selected-family acceptance. The first full attempt's stale OpenRouter card expectation is preserved in `verify-before-fixture-update.log`; the fixture now declares its actual OpenRouter dialect and asserts both raw efforts and effective controls. Magi's initial file-size failure was repaired by splitting model-switch tests into `app/tests/model_switch.rs`, without changing the 800-line limit; its legacy picker test explicitly requires the original six controls and a disabled unadvertised `xhigh`.

The regenerated candidate report now has **21 complete data shapes**. All 869 `xhigh` keys, including nulls, are preserved; **253** records remain pending as `thinkingLevelMap.xhigh:adapter`. Image constraints on 1,290 records, strict-mode compatibility on 980, classifier operation selection and the other reported mappings remain unfinished. The latest retained parity report, `../melchior/target/provider-parity/run-1795212136937913170-366093/report.json`, remains 45 integrations, 10 passing cases, 441 pending and zero live requests. No empty mapping list is provider qualification.

**Magi full-gate fixture repair:** the current full run passed ordinary tests and architecture checks, then failed the hermetic rerun when `FamilyScratch` cleanup removed its shared empty parent between another thread's parent validation and child creation. A new 16-thread/100-cycle regression reproduced the same `ENOENT` before the repair (`target/provider-foundations/scratch-race-repro.log`). `crates/magi-tools/src/family.rs` now serializes local scratch creation/cleanup and bounds retries of parent disappearance caused by another process; privacy checks, cleanup and refusal of other errors are unchanged. Three runs of `oslo make test-family-scratch` pass, including private socket-root checks (`scratch-race-fixed.log`). This is a test-fixture lifecycle repair exposed by the required verification, not a tool-policy change. The original full failure remains in `thinking-verify.log`; full verification must pass again before claiming a new Magi checkpoint.

Magi's fresh complete `oslo make verify` now passes, including the repaired hermetic rerun, Clippy, architectural/wire gates, built-binary family contract, rustdoc and dependency checks. Its retained checkpoint is `target/provider-foundations/run-20261120T223420Z-bZyawG/`; executable SHA-256 `627e0349c7c6fcd055068be6c79c386e082c48b453758cd4919cf433fd8d4c60`. Full/focused logs, the failed hermetic run, the before/after race regression, source archive/digests and a read-only executable are retained. The native build reports dynamic glibc linkage because the musl target is not installed; static-package acceptance remains open. No dependency policy or architecture limit was relaxed.

**Strict tool-schema contract and implementation in progress:** the pinned PI `supportsStrictMode` and Anthropic `supportsStrictTools` fields describe constrained **tool arguments**, not the independent structured-answer `Options.schema` setting. Capability alone does not request strict sampling. Preserve legacy tools unless they explicitly request a provider-neutral `prefer` or `require` policy. `prefer` may retain the original schema when the dialect or schema is unsupported; `require` must fail before credentials, HTTP or retry dispatch. Unsupported adapters and older replacement model roles must not silently accept a required guarantee. This needs a fallible request-preparation contract, matching independent wire types, explicit model-role feature negotiation, tool declaration propagation and actual HTTP fixtures before these compatibility fields can leave the pending report.

Strict conversion closes objects, lists every property as required and makes originally optional non-nullable properties nullable. The source schema must remain unchanged. Tool argument validation must interpret those optional nulls without changing required or explicitly nullable values, defaults, permission checks or transcript arguments. Anthropic additionally rejects constraints accepted by OpenAI. Its upstream preferred-strict fallback is narrowly limited to a classified pre-stream strict-tool-limit rejection; do not add a generic retry/downgrade for arbitrary 400 responses, required tools or started streams. Grammar tools remain a separate pending feature. References: pinned [constrained sampling](https://github.com/earendil-works/pi/blob/5a7479a88390d947335a1a6609936897be0a7891/packages/ai/src/api/constrained-sampling.ts), [Anthropic requests](https://github.com/earendil-works/pi/blob/5a7479a88390d947335a1a6609936897be0a7891/packages/ai/src/api/anthropic-messages.ts) and [tool argument validation](https://github.com/earendil-works/pi/blob/5a7479a88390d947335a1a6609936897be0a7891/packages/ai/src/utils/validation.ts).

**Historical converter-only checkpoint:** `../melchior/src/mind/provider/strict_schema.rs` implements bounded, non-mutating schema conversion and provider-specific rejection, including nested objects/arrays, scalar unions, nullable optional fields and redacted errors. It rejects excessive depth, node counts or string bytes before cloning. Seven tests cover conversion, source preservation, unsupported constructs, Anthropic constraints, annotation-data preservation and limits. `oslo make test-provider-strict`, `clippy` and `gates` pass (`../melchior/target/provider-foundations/strict-schema-focused.log`). At this checkpoint the module was linked under `cfg(test)` only, preserving production request behavior while the earlier family run captures its images. **This checkpoint did not provide runtime strict-tool support**, activate providers or clear the 980/29 pending strict-compatibility records. The first full test attempt exposed a fixture ordering assumption; serde preserves property insertion order, and the expectation now does too. The initial failure is retained in `strict-schema-tests-before-order-fix.log`; the fresh full rerun below now passes. The initial standalone-test module-gate failure is retained in `strict-schema-focused-before-link.log`; the module is now declared normally rather than weakening the gate.

Invoke Oslo recipes individually (or loop over `oslo make "$recipe"`): trailing positional recipe names are not additional targets. Check the emitted recipe headings before treating a multi-command log as evidence.

The strict-converter source checkpoint is retained in `../melchior/target/provider-foundations/strict-schema-20261120T224820Z-3FczBT/`, with source files/digests/archive, tracked patch, passing focused/Clippy/gate/full-test logs and preserved initial failures. Its fresh full `oslo make test` rerun passes, including **849 library tests**, integration tests and example targets (`test.log`, also `../melchior/target/provider-foundations/strict-schema-tests-fixed.log`); it is not a fresh full `verify` receipt. Source and executable checksums have been rechecked. The root run `../target/family/run-T3SFW1lE/` finished all runner fixtures, retained four selected binaries and passed Magi's test command, then failed Casper's library tests: 619 passed, one failed, `mcp_client::http::auth_tests::get_delete_and_external_logout_use_live_bound_credentials`, at the DELETE request. Its final summary records **FAIL**, exit 1 and **zero leaked processes**, with all four artifact integrity checks passing. Later family stages are not accepted. Magi's selected SHA-256 is `627e0349c7c6fcd055068be6c79c386e082c48b453758cd4919cf433fd8d4c60`; Melchior's new selected image is `dba1cb34270431292207d4a9e8c47b088f14c35f1a1e30f44027061de7b46932`, also retained in the strict-converter checkpoint. Do not substitute the earlier Melchior full-verify image's identity for this one.

**Runtime strict-tool checkpoint:** both independent `Tool` types now carry optional `strict: "prefer" | "require"` and both capability types carry optional `strict_tools`; the wire-twins gate compares the tool and policy types as well. Ordinary tools retain their original schemas. Conservative, fieldwise-inherited `supports_strict_mode` / `supports_strict_tools` settings select the existing OpenAI Chat/Responses and Anthropic encodings. Lua owns wire conversion; Rust owns bounded normalization and validation. `Adapter::prepare` is fallible, and the client records preflight refusal without credential refresh, HTTP dispatch or retries. Actual-process tests prove capability cards in JSON/CBOR, policy round-trip, normalized HTTP bodies, preferred fallback, and refusal before an expired OAuth credential can refresh. Structured-answer schemas remain independent; Anthropic's forced-answer-tool conflict refuses required tools rather than silently dropping them.

Magi's broker confirms the selected qualified model's explicit capability using its existing bounded model-role reader before forwarding a required policy. Legacy or unconfirmed roles still accept ordinary/preferred tools but not required guarantees. Lua and command tool declarations propagate the opt-in policy. Both registry execution paths restore originally optional non-nullable nulls to omissions/defaults on a copy; required fields, explicit nullability, the transcript and permission checks are preserved. Traversal is bounded, including properties removed as null. Targeted source scope is recorded above; this is not a new tool permission policy.

Anthropic HTTP 400 `invalid_request_error` responses with one of the pinned strict-limit messages can rebuild and retry once without **preferred** strict schemas. Required schemas remain strict, unrelated errors do not downgrade, a repeated rejection is terminal, and errors inside an already-started stream do not disable strictness. Recovery uses the existing retry ceiling, per-attempt lifecycle and accounting path; budget recovery retains the prepared options. The initial implementation lasted only within one ask. [Pinned PI strict-limit behavior](https://github.com/earendil-works/pi/blob/5a7479a88390d947335a1a6609936897be0a7891/packages/ai/src/api/strict-tool-limits.ts) includes session-scoped memoization; the session checkpoint below now implements that contract, with qualification still in progress.

The regenerated candidate report preserves strict flags and has **119 complete data shapes**. It reports **161** `compat.supportsStrictMode:adapter` gaps and **29** `compat.supportsStrictTools:session_fallback` gaps rather than discarding these values or claiming full parity. Image constraints on 1,290 records, native effort/adaptive behavior, grammar tools, classifier selection and other mappings remain open. Ten catalog tests pass and the checked artifact exactly matches the pinned inputs. Still no runtime catalog importer or new provider activation.

Evidence before the full reruns: Melchior's `strict-runtime-catalog-fixed.log` passes 7 normalizer, 5 adapter, 4 real-HTTP client recovery and 2 actual-process readiness tests, 10 catalog tests, Clippy and architecture gates. Magi's `strict-runtime-native-fixed.log` passes the replacement-role negotiation test, two optional-null tests and the Lua/command registry propagation test, with check/Clippy/gates; an additional null-removal traversal regression is included in the fresh run. The subsequent commands were individual `oslo make fmt`, `test-provider-strict`, then `verify`, logged at each repository's `target/provider-foundations/strict-runtime-verify.log`. Both commands are now terminal; their distinct outcomes follow. Cross-program selected-image acceptance and the prior Casper failure still require follow-up.

The fresh Magi run passed all **five** focused strict tests, including the traversal-bound regression, then failed full verification: the host library reported **592 passed, one failed**, `broker::strict::tests::required_policy_is_negotiated_before_asking_a_replacement_model_role`. Starting its temporary model-role executable returned `Text file busy (os error 26)`. The command exited 1; no fixture fix or full rerun is accepted yet.

Melchior's fresh full `verify` completed successfully: **858 library tests** in each default/all-feature run, integration suites, Clippy, rustdoc, architecture and hermetic gates, built-binary family/role checks and dependency/advisory checks. The retained checkpoint is `../melchior/target/provider-foundations/strict-runtime-20261120T231440Z-oqyMyn/`, containing source archive/digests, tracked patch, logs, result and executable. Its binary SHA-256 is `c1b54f8a765d5c32335bc0bd96256b04bd20c89857c3035754b4b7d9c9e7fbc2`. This checkpoint explicitly lacks cross-turn fallback and the subsequent fingerprint repair; it is not selected-family or live-provider acceptance.

After that archive, an adapter regression demonstrated that strict and downgraded requests incorrectly shared a provider-policy fingerprint (`strict-policy-fingerprint-repro.log`). The private fallback option is now included in `../melchior/src/mind/lua/adapter/frame.rs`'s fingerprint. The focused rerun exited 0 with **7 normalizer, 6 adapter, 4 HTTP-recovery and 2 native readiness tests** passing (`strict-policy-fingerprint-fixed.log`). Full verification of this newer source remains pending.

**Next execution checkpoint — family proof and remaining catalog semantics:**

1. Diagnose the terminal selected-family failure in `run-T3SFW1lE` before rerunning `oslo make test-family`. The unchanged Casper fixture closes each HTTP/1.1 connection after one reply without declaring `Connection: close`; stale connection reuse is a hypothesis, not yet a proven cause. The focused `oslo make test-mcp-auth` rerun passes (37 OAuth, 7 HTTP-auth, 10 identifier and 12 native OAuth tests plus mutation gates; `../casper/target/provider-foundations/family-auth-repro.log`), so it did not reproduce the full-suite failure. Casper remains unmodified. Preserve the full failed receipt and repair a fixture only if its defect is demonstrated, without relaxing authentication or containment checks. No full-family verification process remains in flight; the newer focused strict-family receipt does not exercise this failing auth case.
2. Finish Magi's fresh full `verify` after the fixture repair and session-fallback work. Keep the earlier failed receipts. The selected-family strict contract now passes, so its 29 session-fallback catalog markers are removed and catalog reproducibility tests pass. Map image constraints and keep unsupported native effort/adaptive semantics explicit; regenerate and test after every mapping change.
3. Introduce operation-qualified classifier identity/selection and a reviewed runtime catalog importer. Preserve all 1,779 in-scope records and both existing extra integrations; do not enable unreviewed candidates as working providers.
4. Bring `describing.rs` under bounded authenticated I/O and complete bundled/offline startup acceptance. Then activate compatible API-key providers in qualified batches with per-provider fixture evidence.
5. Keep full parity, auth/cloud protocols, actual UI/ACP acceptance, static packaging and authorized live receipts open. Passing reasoning tests or full local verification does not complete those workstreams.

**Still required:** finish routing/readiness/UI/regression verification; complete reference-model/discovery review; all catalog/auth/cloud/transport/provider expansion phases; complete offline qualification; selected-image/UI/static-package acceptance; explicitly authorized live receipts. Shipped provider breadth remains nine entries at this checkpoint.

**Session-fallback implementation checkpoint (2026-11-21):** both independent `Wants` and `RequestFrame` types now carry optional `disable_preferred_strict`, false by default and omitted when false. The twins gate also compares `RequestFrame`. Melchior propagates the option into preparation and reports its effective value; required tools bypass the preferred-disable filter. Magi's owning `Session` keeps at most 4,096 hashed `(model-role program, qualified model)` identities, deduplicated with oldest-first eviction. It learns only version-1 feedback against the in-flight request identity, not the current UI selection. The effective option is applied to both the next request and host cache observation; provider fingerprints include it too. New sessions, resume and restart clear the ephemeral state; it is never journaled or shared across sessions. There is no vendor-specific Magi client or cross-program shared crate.

- Magi's temporary role fixture now probes execution before exercising negotiation, retrying only OS error 26 within a five-second deadline. A test holds an actual writable file descriptor, observes the kernel refusal, closes it and proves the fixture becomes executable; unrelated spawn errors remain errors. This reproduces the busy-writer condition but does not identify which original parallel child held the descriptor. The six focused strict tests passed in `strict-fixture-busy-fixed.log`; `strict-runtime-verify-before-busy-fix.log` preserves the failed full run.
- Melchior's new actual-process test captures a strict-limit failure, preferred-only retry, feedback and a second process's first HTTP request using that feedback. Required tools remain strict and unchanged; JSON/CBOR carry the additive fields. `strict-session-focused.log` passes **20 tests** (7 normalizer, 6 adapter, 4 HTTP recovery, 3 native readiness), and `strict-session-gates.log` passes Clippy/gates. Fresh full `verify` passed, including **859 library tests** in each default/all-feature run, integration suites and all subsequent gates. The retained source/binary/log checkpoint is `../melchior/target/provider-foundations/strict-session-20261120T233325Z-ACppO2/`, binary SHA-256 `fe64bfb2946586c310ac4b333b516ce7af5c5ccc433b9a398a3d7f25e3162b6c`. It predates only the subsequent catalog-marker removal and documentation update; those maintenance changes pass ten catalog tests, deterministic regeneration, Clippy and gates in `strict-session-catalog.log`.
- Magi now tests bounded/session/model/role/version/resume behavior and real turn-loop propagation with cache events and transcript non-persistence. Fresh `check`, **12 focused strict tests**, Clippy and gates all pass (`strict-session-focused.log`). An initial missing direct CBOR test dependency was fixed by using existing `magi_ipc::Wire`, without adding a dependency. The existing CBOR round-trip helper is reused rather than duplicating serialization inside the size-constrained protocol crate; its unchanged size gate passes at 4,000 lines. Initial compile-failure logs are retained as `strict-session-before-codec-fix.log` and `strict-session-before-cbor-helper-fix.log`. Full `verify` subsequently failed in `events::collection::runtime::branches::tests::panicking_tree_worker_fences_admission_closes_pending_hosts_and_is_joined`, at the `/proc` task-disappearance assertion: **596 host library tests passed, one failed** (`strict-session-verify.log`). The test's source is unchanged. A subsequent complete `test-events` recipe passed, including that test (`strict-session-events-recheck.log`, exit 0); the cause is not established and full verification remains unaccepted.
- Root `oslo make test-provider-strict-family` selects the optional `provider-strict` scenario through the existing isolated acceptance runner. The fresh run **passed** at `../target/acceptance/run-nqCJ2rDZ/`, with actual request/event/replay captures and ten rejected mutations. All four selected binaries and both fixtures passed retained-artifact integrity checks. Magi's selected SHA-256 is `0f45d81b35ad9c155bfac1782781e5f08d133228e7dc02d7bd4dd00b796dc2e5`; Melchior matches the full-verify image above. Four HTTP requests prove the initial strict rejection, preferred-only retry, following tool round and next prompt; required tools stay strict. Provider diagnostics show `provider_policy_changed`, and the next host request reports `options_changed` exactly when fallback becomes effective. The original null argument remains journaled while execution receives its default. This is scenario-scoped acceptance, not the full family suite, live entitlement or static packaging.
- The first strict-family run failed (`../target/acceptance/run-EIlM07Ij/`): fixture tools were placed in `init.lua`, whose declarations are not replayed into the worker's tool VM. The fixture now uses the documented `plugin/` discovery path and verifies both tools are listed before opening the session. No production configuration-loading behavior was changed. The failed receipt and `strict-session-family-before-plugin-fix.log` remain retained. The current outer log is `../target/provider-foundations/strict-session-family.log`.
- Magi's source/binary/focused-proof snapshot is `target/provider-foundations/strict-session-source-20261120T233628Z-7wac8a/`. It includes the selected-family summary and a separate archive/digest list for the five provider-acceptance runner/verifier files. Its recorded full-verification status was **RUNNING** when captured; the completed run failed as recorded above, so that snapshot is not a full-verification pass. Both this source archive and the Melchior full-verify source archive were extracted and checked against their recorded digests; subsequent importer, documentation and catalog changes are distinct from those snapshots.

**Catalog-import checkpoint (2026-11-21):** explicit `catalog = {source = "pi-1.6.0", provider = "groq", models = {"chat:llama-3.3-70b-versatile"}}` selections now enter the existing keyed provider declaration pipeline. Unknown, duplicate, pending and unsupported-operation selections refuse atomically; local model overrides remain subject to routing, credential-origin and metadata validation. Derived capabilities are recomputed after overrides. `../melchior/target/provider-foundations/catalog-imports-focused.log` records **six passing unit tests, three passing native cases, ten catalog checks, deterministic regeneration, Clippy and all architecture gates**. The refusal fixture now checks the single typed `failed/invalid` event, no network request and unchanged credentials; production refusal exit semantics were not changed. The failed fixture log is preserved as `catalog-imports-before-refusal-fixture-fix.log`. The API is documented in Melchior's `EXTENDING.md` and `catalog/README.md`; no upstream code or startup download is introduced.

**First API-key batch (2026-11-21):** `groq` selects `openai/gpt-oss-120b` and `openai/gpt-oss-20b`; `cerebras` selects `gpt-oss-120b`. All use the pinned importer and bundled discovery. Their source/default review, documentation links and remaining six reference records are recorded in `../melchior/catalog/activations.md`. The reference inputs and full target are unchanged. `api-key-presets-final-focused.log` passes the two reasoning-field unit regressions and three native test groups across every selected model, plus the parity harness. Earlier batch Clippy/gates pass in `api-key-presets-focused.log`. The exact usage fixture is 60 uncached input, 40 cached input and 20 output tokens; estimated charges are 24, 12 and 50 microdollars for Groq 120B, Groq 20B and Cerebras 120B respectively, with no falsely reported provider charge.

- The Chat Completions adapter now records `reasoning`, `reasoning_content` or `reasoning_text` once as the thinking signature and replays the same field. Other opaque signatures cannot create JSON keys; unsigned legacy thinking retains its previous field. The failing before-fix regression is retained in `api-key-presets-before-reasoning-field-fix.log`. Native tool-round requests prove the `reasoning` field survives actual model-role serialization.
- The parity harness now uses production catalog assembly instead of raw provider deserialization that would ignore selectors. The latest receipt is `../melchior/target/provider-parity/run-1795219255065317789-2513851/`: **45 target integrations, 11 bundled declarations, 14 passing protocol smoke cases, 437 pending and zero live requests**. Receipts include the selected model inventory and generated-catalog digest. Both parity recipes depend on the native preset recipe. Authentication/tool-loop/discovery cases in the complete parity report remain pending until their full required scope is integrated, despite the narrower selected-model tests passing.
- Melchior full `verify` **passes** against the frozen source snapshot `../melchior/target/provider-foundations/api-key-presets-20261121T000159Z-Gyu8Tp/`; both default/all-feature library runs pass **867 tests**, followed by integration, Clippy, rustdoc, architecture, hermetic, build, family/role and dependency/advisory checks. The receipt records exit 0, checked source digests and zero live requests. Its retained executable SHA-256 is `64c73db7418a98353c53bb8f1ff9b0f97f220e01d56bf9ad5432df892698646e`; this dynamic executable is not static-release acceptance and predates the input-limit implementation below. Magi's earlier full run ended **FAIL** in `target/provider-foundations/catalog-imports-magi-verify.log`: its initial tests and architecture gates passed, but the hermetic rerun had **595 host library passes and two failures**, the tree-worker `/proc` assertion and capability-query fixture. The separate test-only repair/diagnostic recipe **passes** in `catalog-imports-events-diagnostic.log`. Its fresh full run now also **passes** in `catalog-imports-magi-fixtures-verify.log`, including hermetic, built-binary family, documentation and dependency/advisory checks. No production cleanup or deadline was relaxed. Cancellation, cross-provider handoff, remaining catalog mappings, complete family/UI/static acceptance and authorized live evidence remain open.

**Input-limit checkpoint (2026-11-21):** optional positive-valued `InputLimits`, `ImageInputLimits`, `ImageResize` and bounded JPEG quality now survive native model serialization. Invalid/unknown declaration fields refuse without echoing values. The compiler preserves every source limit on all **1,290** affected records. The pinned semantics are defined in [PI's model schema](https://github.com/earendil-works/pi/blob/5a7479a88390d947335a1a6609936897be0a7891/packages/ai/src/providers/model-schema.ts): request bytes mean serialized provider JSON, image counts refer to provider-message boundaries, and resize byte limits measure base64 payloads. Resize profiles belong before new images enter history.

- `../melchior/src/mind/provider/client/input_limits.rs` checks the final body after removing internal budget/scratch fields, before authentication or dispatch. Counting JSON bytes uses a bounded non-allocating writer and matches captured HTTP bytes, including UTF-8 and escaping. Image traversal is protocol-specific and excludes tool arguments; it includes merged user/tool messages and lifted tool images. Oversized input returns a terminal `invalid` error without retries, credential refresh, network access or content/credential echo. Rebuilt fallback requests pass through the same checks. Text-only image omission remains before these checks and does not mutate history.
- `oslo make test-provider-input-limits` passes **two metadata tests, seven transport/conversion tests and two actual-binary tests**. The latter prove no OAuth refresh or credential-store mutation on all four enforced limit categories and safe invalid-declaration diagnostics. `test-provider-catalog` passes **thirteen tests**, including every pinned limit value and deterministic regeneration. Importer regressions pass six unit/three native tests in `input-limits-focused.log`. Final focused, catalog, preset/parity, Clippy and architecture checks pass in `../melchior/target/provider-foundations/input-limits-qualified.log`. Initial PI image-shape and Google merged-tool-message expectation failures remain in `input-limits-first.log` and `input-limits-final-focused.log`; both are covered by the passing suite.
- This does **not** implement resizing, dimension validation or propagation into Magi's model metadata. The generated report retains **1,290** `inputLimits.images.resize:history` gaps and **163** `inputLimits.images:adapter` gaps for Bedrock. All 1,779 in-scope identities, 43 provider IDs and **119 complete data shapes** remain; no additional provider/model is activated by this checkpoint. The new parity receipt `../melchior/target/provider-parity/run-1795220622491620486-2776625/` still reports **14 protocol smoke passes, 437 pending and zero live requests**.
- Full Melchior verification **passes** against `../melchior/target/provider-foundations/input-limits-20261121T002416Z-69Fxvx/`, including **876 library tests** in each default/all-feature run, integration, Clippy, rustdoc, architecture/hermetic, built-binary family/role and dependency/advisory checks. Source digests match the pre-run archive; the retained binary SHA-256 is `5a5ffb6aa0bd38513983d854ba02d2602557aa727b4ff11358f823e1e13860d0`, rechecked after completion. This snapshot predates the metadata changes below. Magi's completed full verification is retained separately at `target/provider-foundations/magi-fixtures-20261121T002330Z-oZ9MsI/`, with logs and a source archive captured after verification, not a pre-run source freeze. Its executable SHA-256 remains `0f45d81b35ad9c155bfac1782781e5f08d133228e7dc02d7bd4dd00b796dc2e5`; the event-fixture changes are test-only. Full family, static packaging, UI and live-provider acceptance remain open.
- Next: expose the same optional input-limit metadata through the independent Magi/Melchior wire, then normalize newly admitted user and tool images before journal storage. Preserve existing bounds and frozen-history bytes; model switches to stricter limits must not silently recompress historical images. Do not remove the history markers until those paths, resume/replay, cache stability and actual image dimensions have been verified.

**Input-limit metadata checkpoint (2026-11-21):** both independent model libraries now carry optional `Capabilities.input_limits` with the same positive-valued types and bounded JPEG quality. Melchior derives advertised limits from the model's enforced `input_limits`; contradictory explicit capability metadata refuses the declaration. Legacy cards omit the new field, and no shared Rust crate or protocol revision was introduced. The generator preserves the profile in derived capabilities; the embedded report is 3,756,326 bytes, still below its existing 8 MiB bound.

- Melchior passes **fourteen** focused input-limit tests: two type tests, one authoritative-metadata test, seven conversion/transport tests and four native tests. Real CLI listing preserves all values in JSON and CBOR without credentials or network activity. Thirteen catalog tests and deterministic generation pass; importer regressions pass six unit/three native cases. Preset/parity, Clippy and architecture gates pass. Logs: `../melchior/target/provider-foundations/input-limit-metadata-{focused,final}.log`. The latest parity receipt is `../melchior/target/provider-parity/run-1795221152003960036-2878443/`, still **14 smoke passes, 437 pending, zero live**.
- Magi passes **five unique** focused tests: two type tests, two listing/encoding tests and an isolated native host-session test. The session test covers two attached clients, a changed resize profile, atomic rejection of a zero limit, fresh snapshots and switching to/from a legacy model without metadata. `ModelInfo` remains only name/window; snapshots resolve that name into their existing rich `choices` list. `check`, Clippy and all architecture gates pass in `target/provider-foundations/input-limit-metadata-magi-final.log`; the 4,000-line protocol-crate limit is unchanged. Earlier logs preserve a missing test initializer, a mistaken `ModelInfo` assumption and a test-only broker-to-catalog cycle; the corrected tests preserve layering instead of weakening gates.
- `scripts/gate-twins.sh` now compares the input-limit declarations and bounded scalar conversion, not just field spellings. In `target/provider-foundations/input-limit-twin-mutation-LZC4Tf/mutation.log`, a copied Melchior definition accepting JPEG quality 101 is rejected by **only** `gate-twins`, with every other gate passing. Production definitions were not mutated.
- These are focused metadata proofs, not fresh full-verification or selected-family receipts for the newer source. `ROLES.md`, Melchior `EXTENDING.md` and `catalog/README.md` describe the additive metadata and remaining normalization gap. Provider counts and all **1,290 history / 163 Bedrock** pending markers are unchanged.
- Next implementation: route image-bearing user/workflow requests through bounded preparation before journal admission, normalize tool images before recording results, and reject incompatible frozen-history images without rewriting them. Existing attachment count/byte limits, cancellation, configuration epochs and queue bounds must remain in force. Relevant ingress points are `crates/magi-host/src/{preparing,turning,turn}.rs` and `crates/magi-tools/src/bound.rs`; workflow validation also runs in `skills/snapshot.rs`. Casper already has a separate raster processor in `../casper/src/images.rs`; no Casper changes or codec dependency were made here. Do not treat the image library's best-effort allocation limit as a hard memory guarantee; assess strict dimensions, decoded-size bounds and execution isolation before adopting a codec ([image limit contract](https://docs.rs/image/latest/image/struct.Limits.html)).

**Image-worker checkpoint (2026-11-21, integration still pending):** `crates/magi-host/src/images/` now implements raster normalization and immutable validation. `magi --image-worker-v1` dispatches one bounded JSON operation before configuration, Tokio or family startup. The workspace pins the resolved `image` 0.25.10 dependency in `Cargo.lock`, with only PNG/JPEG/GIF/WebP/BMP codecs enabled.

- Images that fit keep their exact bytes. New oversized images are oriented, downscaled without upscaling, and encoded within both the base64 profile and existing 128 KiB inline limit. PNG preserves transparency; opaque images try PNG/JPEG before reducing quality or dimensions. A too-small profile fails atomically. Validation refuses incompatible historical images instead of recompressing them. Animated GIFs stay byte-identical when fitting; resizing uses the first frame, not a preserved animation. The pinned [PI resizing implementation](https://github.com/earendil-works/pi/blob/5a7479a88390d947335a1a6609936897be0a7891/packages/coding-agent/src/utils/image-resize-core.ts) is the behavioral reference, not a runtime dependency.
- Decoding has strict 8,192-pixel edges, an 8 Mi-pixel ceiling and a 64 MiB decoded-output ceiling. The codec's 64 MiB allocation setting is only best effort ([library contract](https://docs.rs/image/0.25.10/image/struct.Limits.html)). The worker separately sets hard address-space/CPU limits of 256 MiB/four seconds and zero core/file-output limits, never raising stricter inherited limits. These are resource controls, not a filesystem/network sandbox claim.
- The parent executes its own `/proc/self/exe`, clears the child environment, uses nonblocking bounded pipes and permits at most two workers. The eight-second wall deadline includes waiting for a slot. Cancellation, timeout and ordinary completion kill the owned process group and reap its leader. Requests/replies are limited to 1 MiB; errors do not echo image data.
- `oslo make test-provider-image-worker` passes **21 host tests** (20 new worker tests and one existing history reconstruction test) and **five native CLI tests**. These cover actual pixel dimensions/colors, transparency, EXIF orientation, exact no-op bytes, format/profile refusal, cancellation/reaping, timeout, bounded pipes, strict dispatch, hard process limits and preservation of stricter inherited limits. `check`, Clippy and all architecture gates pass in `target/provider-foundations/image-worker-wire-final.log`. Initial fixture failures remain in `image-worker-focused.log`: Nix fixture utilities now use explicit paths after environment scrubbing, and the pixel-budget fixture fits the unchanged inline bound. Earlier logs retain a corrected integration-helper lint failure and the wire gate's rejection of a nonstandard `status` tag. Worker replies now use the existing `event` convention; the gate was not weakened.
- The focused source/binary receipt is `target/provider-foundations/image-worker-20261121T010143Z-suMdOW/`, with checked source digests and executable SHA-256 `e1f18899fcfa2d2b864907015bc594b09e5a8d5f437622c767270c7fa5b99df8`. Repeating the recipe under a dedicated `TMPDIR` passes 21 host tests and five native tests; the native tests invoke the copied executable. The scratch directory is empty afterward, and executable integrity is rechecked. `machete`, offline `audit` and `deny` pass in `image-worker-dependencies.log`; existing allowed advisory/duplicate warnings are retained, with no policy relaxation. This is a dynamic executable and focused acceptance only, not a fresh full-family/full-verify/static-package receipt.
- No user/workflow admission, tool-result journal or replay path calls the worker yet. No provider activation or pending marker was changed: **11 declarations, 1,290 history gaps, 163 Bedrock image-adapter gaps** remain. Next is bounded pre-admission integration, current-model/refresh fencing, tool-result handling and immutable resume/replay/cache proof; worker tests alone do not qualify those paths.

**Image-history integration checkpoint (2026-11-21, focused Magi proof):**

- Image-bearing user and workflow prompts enter the existing bounded preparation queue before admission. Their reservations include up to 1 MiB of normalized reply expansion. Cancellation, epoch changes and the existing preparation deadline cover decoding and workflow lookup. The selected profile is checked again under the session lock at admission, including refreshed model metadata.
- Tool-result images normalize before journal amendment. Failure omits the whole image batch, retains text and marks the result failed. Current-profile and retired-session fences apply before storage. The repeated-call guard now records final failure status once, using the original validated output fingerprint from before masking/spill paths; generated refusals do not count as fresh completions. Memory feedback and file delivery use the final failure status.
- Each prompt keeps at most 512 successful history-validation digests, keyed by image bytes, media type and every resize-profile field. History is checked in bounded worker batches before model dispatch, with an eight-second whole-history deadline; no old image is rewritten. Tool arguments are not traversed as attachments. A stricter refresh/model selection can refuse incompatible history; text-only or absent-profile paths preserve existing bytes.
- Pending-store byte accounting now includes user and tool image payloads/media types as well as workflow images. The existing 64 MiB/2,048-entry pending limits are unchanged.
- `oslo make test-provider-image-history` passes **50 host unit tests** (26 image-related, 12 preparation, five pending/resume and seven progress tests), **48 turn regressions** and **five native CLI tests**. The native fixture uses disposable HOME/XDG/project/runtime directories, selected Balthasar/Casper artifacts, a synthetic model peer and actual Magi. It proves prompt/tool/workflow storage, exact-session process restart/resume, metadata refresh, model switching, invalid pixels, unrepresentable tool images and repeated image failures across rounds and within duplicate batches. `check`, Clippy and all architecture gates pass in `target/provider-foundations/image-history-final.log`.
- Initial native failures remain in `image-history-focused.log` and `image-history-qualified.log`. Fixture corrections use `--resume-run` for a named session (Boolean `--resume` otherwise leaves the ID as a new prompt), stage a read-only single-link bubblewrap launcher using the existing workflow-test convention, and acknowledge model refresh with `refreshed: true`. Neither containment nor refresh validation was relaxed.
- The frozen source and selected-binary receipt is `target/provider-foundations/image-history-20261121T014438Z-3Y8dJN/`. Both image recipes pass again using its copied Magi executable, SHA-256 `3ecb0a90b2ecd598c9d9aa93bcfa0e6113949613cb6cae9c18aa64926ba51b8a`; the worker recipe additionally passes five native resource/protocol/pixel tests. Source and selected peer digests match afterward. The short dedicated temporary directory retains only the two expected spill-output fixtures from the progress regression, archived under `scratch/`; native fixture directories are gone. The executable is dynamic, not a static release proof. No fresh full-family/full-verify or live-provider acceptance is claimed.
- No provider activation or catalog marker has changed. Next: selected-family image transport/cache/retry acceptance with actual Melchior, the remaining cancellation/racing-refresh integration cases, then review whether the **1,290** history markers can be cleared. Bedrock's **163** image-adapter gaps remain separate. Full verification and static-package qualification must cover the newer source before release.

**Selected-family image transport checkpoint (2026-11-21, Anthropic only):** root `oslo make test-provider-images-family` passes at `../target/acceptance/run-cwi76y1D/`. It uses the existing private test context, native build/selection recipes, disposable configuration and a local HTTP fixture; no account credentials or external inference are involved.

- A generated 64×32 BMP is normalized to a 20×10 PNG within a 1,000-character base64 limit. Captured HTTP bodies and Balthasar replay prove identical normalized bytes for both the user attachment and Lua-tool result. Five requests include one HTTP 503 retry, the tool round, a resumed prompt and a later prompt. The retry body is identical; historical prefixes and signed thinking remain unchanged. Cache-marker legality and diagnostic boundaries are checked, not real server-side cache hits.
- `../scripts/provider-images.jq` checks the request/event/journal contract; `../scripts/tests/provider-images.sh` rejects 15 mutations, including omitted/changed images, altered signatures, changed prefixes, missing diagnostics, invalid markers and wrong/missing running-image identity. Captures, fixture scripts, normalized pixels and mutation results are retained. The earlier uninstrumented pass remains at `../target/acceptance/run-2rDi1ZUZ/`.
- `/proc/<pid>/exe` path and hash checks match the selected **Magi, Melchior and Balthasar** processes. Casper is built and retained but the image-producing tool runs in Magi's Lua layer; this is not runtime Casper image-transport proof. Selected Magi SHA-256 is `e359f9ea5d7077a8c73a7a18d0ca03df6327abf8aa301cccb9097877960b55bf`; Melchior is `fb40e9300279159e3665e869593c45a291188800002d6ad342a518fbc1e848ef`. All four family artifacts and both fixtures pass final integrity checks. These are dynamic native executables, not static-package proof.
- Environment failures are retained: `run-s44Pxfg3` stopped because Balthasar's cached development PATH referenced a missing pinned Rust toolchain; entering its declared Nix shell restored that toolchain and `build-host` passed. `run-P6H0hlbb` rejected a Nix profile directory containing an installed Magi. The acceptance runner now resolves utility executables to their actual directories, verifies the same named command exists there, and preserves the refusal of directories containing unselected family binaries. The initial identity fixture at `run-iUrVK5ax` incorrectly required a persistent Casper process; the corrected contract names only the three roles actually used by the image path. No containment or selected-image check was waived.
- The root deny-by-default `.gitignore` now explicitly admits both provider scenarios and their verifiers/tests, so they are deliverable source files rather than ignored local fixtures. Broader adapter coverage and fresh full gates remain open. All **1,290** history markers and **163** Bedrock image-adapter gaps remain unchanged.

**Native preparation-race checkpoint (2026-11-21):** `crates/magi-cli/tests/image_history/racing.rs` adds two actual-process tests. They discover only the selected host's children, pin a candidate with a PID file descriptor, verify parent, executable and worker argument, and stop that worker while processing a bounded 4,000×2,000 image. Signals use the pinned descriptor; cleanup resumes a still-held worker and the owning fixture reaps its processes.

- Interrupting preparation produces a tracked refusal, reaps the stopped worker, leaves history empty and sends no model request. A subsequent image prompt completes successfully.
- Refreshing the selected model from 20-pixel to 10-pixel limits while preparation is stopped rejects the old prompt at admission after the worker continues. No stale image is stored or dispatched. The next prompt and its tool result both store 10×5 images.
- `oslo make test-provider-image-history` now passes **50 host unit, 48 turn and seven native CLI tests**. Three further runs with `MAGI_REQUIRE_LIVE=1` use preserved Magi/Balthasar/Casper binaries and a copied Casper configuration; every run passes all 105 tests. `check`, Clippy with warnings denied and all architecture gates pass. These race tests use a synthetic model peer; the separate Anthropic HTTP receipt above supplies actual Melchior transport evidence.
- Receipt: `target/provider-foundations/image-family-20261121T021101Z-yB4C8z/`. It retains Magi/Melchior/family test sources and hashes, selected binaries, gate logs, all repeat logs and the family result. Source and executable hashes match after the repetitions. No runtime provider implementation changed in this checkpoint. Next: qualify the other implemented image adapters and audit tool/history cancellation coverage before reviewing the history markers; then run fresh full and static-package gates. This does not resolve the previous full-family Casper failure or any live row.

**Multi-protocol image qualification and catalog checkpoint (2026-11-21):** `../target/acceptance/run-LDGSiLGx/` passes all eight implemented image adapters: Anthropic Messages, Chat Completions, Responses, Azure Responses, Codex Responses, Gemini, Vertex and PI Messages. Each protocol performs the same five real HTTP requests through selected Magi/Melchior binaries, including a 503 retry, image-producing tool round, resume and continuation. Each verifier rejects 15 corruptions, for **40 requests and 120 rejected mutations**. Runtime image-path identities and durable normalized bytes are checked for every protocol. Azure/Vertex/Codex here mean local adapter serialization, not production service routes, authentication or subscription entitlement. Bedrock and classification-only Decisions are not included.

- The fake provider has an explicit bounded `raw-sse` fixture mode for literal response frames; it does not translate or substitute for production adapters. The Responses corpus follows the [streaming event reference](https://developers.openai.com/api/reference/resources/responses/streaming-events); the Gemini corpus uses [GenerateContent response fields](https://aistudio.google.com/api-reference/generate-content). The PI Messages corpus tests the existing local adapter contract. Thirty-two fixture tests pass, including byte-split decoding, one terminal event, tool arguments, usage, frame/name bounds, mixed-dialect refusal and chunked HTTP. The first compile failure from an incorrect test-module path is retained in `../melchior/target/provider-foundations/image-protocol-fixtures.log`; the corrected run is `image-protocol-fixtures-fixed.log`.
- The first matrix run, `run-0IwKOhIN`, retains three failures: the client finished streaming before the independent Balthasar replay query observed the last stored answer. The scenario now waits at most ten seconds for that exact stored assistant completion, bounds individual replay commands to five seconds, and retains each observation. The exact eight-entry/prefix/byte verifier is unchanged; partial durable history is never accepted.
- Native cancellation now also covers tool-result normalization and historical validation, using the same pinned-PID worker synchronization as the preparation tests. Cancelling a tool image records a failed result without images, retains text, reaps the worker and prevents a following model request. Cancelling historical validation preserves stored bytes, reaps the worker and prevents dispatch; a later prompt still succeeds with the original images. `image-history-cancellation-paths.log` passes 50 host, 48 turn and nine native CLI tests; `image-history-cancellation-gates.log` passes check, Clippy and all Magi gates.
- The compiler no longer emits `inputLimits.images.resize:history`. Reproducible generation preserves every input-limit value on all **1,290** affected records; the artifact is **3,691,127 bytes**, with **313 complete shapes**, 43 unreviewed providers and zero automatically activated providers. All **163** `inputLimits.images:adapter` gaps remain, as do provider-specific image/header/reasoning/affinity restrictions. A complete shape is not a supported provider.
- Thirteen catalog tests and reproducibility checks pass. Seven importer unit tests and three native import tests pass, including positive selection of a newly complete image profile and refusal of still-pending records before authentication or network access. The first importer failure is preserved in `image-history-catalog-qualified.log`: Groq's Qwen fixture had become legitimately complete. The negative test now selects the same model under OpenRouter's unresolved affinity mapping; the refusal itself was not relaxed. The corrected catalog/import/preset/parity/check/Clippy/gate receipt is `../melchior/target/provider-foundations/image-history-catalog-refusal-fixed.log`.
- Generic parity remains **14 passed, 437 pending, zero live requests**, at `../melchior/target/provider-parity/run-1795228176488638600-4003796/`. No bundled provider/model selection changed. Next: fresh full/static qualification and the next reviewed compatible-provider batch; retain the previous full-family Casper failure until independently resolved.
- After regenerating the embedded catalog, the full eight-adapter scenario passes again at `../target/acceptance/run-YAv677ez/`, with 40 requests, 120 rejected corruptions and final integrity checks. The rebuilt Melchior SHA-256 is `b7ebead0a53239f7f61287be8b51f441aa7e0f14e05fcc0e383999ee61b75540`; Magi remains `e359f9ea5d7077a8c73a7a18d0ca03df6327abf8aa301cccb9097877960b55bf`. A second native image-history run passes all **107** focused tests with required selected peers (`image-history-cancellation-repeat.log`). The frozen source/binary/log checkpoint is `target/provider-foundations/image-protocols-20261121T023310Z-uQDuAx/`. Both family runs are scenario-scoped, not whole-family or static-release acceptance.

### NVIDIA selected-model checkpoint (2026-11-21)

- Added `nvidia` with `stepfun-ai/step-5-preview`, `NVIDIA_API_KEY`, bundled discovery and the pinned NVCF header. Current breadth is 12 declarations and four selected imported models across Groq/Cerebras/NVIDIA; the other 11 NVIDIA records and all other reference IDs remain in scope. The [NVIDIA Step 5 API](https://docs.api.nvidia.com/nim/reference/stepfun-ai-step-5-preview-infer) documents eight images and always-on reasoning. A local override preserves the pinned resize profile, bounds images request-wide and advertises no effort controls. See `../melchior/catalog/activations.md` for provenance and unresolved qualification.
- `supportsLongCacheRetention: false` is now recognized only for Chat Completions and Responses, whose actual requests omit the corresponding fields. True, malformed and other-protocol values remain pending. Reproducible conversion retains all 1,779 records: 333 complete data shapes, 107 fewer retention markers, 16 retention markers remaining, zero automatic activations. Source values and other compatibility restrictions are unchanged.
- Native pricing evidence exposed an unconditional zero estimate for wholly unknown rates. `running::Tally` now omits that estimate. The first attempted fix also dropped positive partial estimates and failed three spending regressions; the corrected implementation retains those amounts and passes all 12 native spending tests, including retry tiers, settlement and cancellation. This does not make unknown prices safe for a complete dollar cap. Operational accounting's separate unknown-rate semantics and the spend ledger are unchanged.
- Five NVIDIA native tests cover offline metadata/refresh and credential isolation; reasoning, split tool arguments and seven-user-plus-one-tool images; nine-image and strict-required refusal before auth; 400/401/422 nonretry failures; and byte-identical eight-image 503 retry. The preset recipe passes eight native tests plus two shared reasoning tests. The retry fixture accommodates the existing 8–10 second backoff with explicit 15/25-second server/process deadlines; production timeouts are unchanged.
- Test-only JSON control decoding removes empty-array tags. The NVIDIA loopback overlay restores the explicit empty thinking-controls array, and the parity harness leaves already-bundled declarations untouched rather than needlessly round-tripping them through that decoder. Earlier module-path, array-shape and retry-deadline failures are retained in `../melchior/target/provider-foundations/nvidia-*.log`.
- The current parity receipt is `../melchior/target/provider-parity/run-1795229417668396062-960/`: **16 text/usage smoke cases passed, 435 pending, zero live**. Catalog tests (14 plus exact regeneration), importer tests (seven unit/three native), native presets and usage/spending checks pass. Remaining verification for this source is tracked in the final evidence bullet below; no full-family/static/live claim is implied.
- Fresh Melchior `oslo make verify` passes: **879 library tests in each default/all-feature run**, integration suites, rustdoc, architecture gates, repeated hermetic suite, family-interface/model-role checks, dependency usage, audit and deny. Separate phase-03 streaming, overflow and cache-contract recipes pass. Magi's model-refresh recipe also passes. Logs: `../melchior/target/provider-foundations/nvidia-full-verify.log`, `nvidia-batch-offline-fixed.log`, `nvidia-partial-price-preserved.log`, and `target/provider-foundations/nvidia-model-refresh.log`. The built Melchior release image is dynamic, SHA-256 `cc084d803367584c967b28315ca23f102027fbee1cb80e1f8434246607a62d65`; this is local Melchior verification, not fresh whole-family, selected-provider release HTTP, terminal/UI, static-package or live acceptance. Existing family-script null-byte substitution warnings remain visible in the receipt.
- Next compatible-provider work must preserve actual dialects: pinned xAI uses Responses, not Chat Completions, and also declares subscription OAuth. Its [Responses reference](https://docs.x.ai/developers/rest-api-reference/inference/responses) defaults to storing responses. PI's [pinned Responses request builder](https://github.com/earendil-works/pi/blob/5a7479a88390d947335a1a6609936897be0a7891/packages/ai/src/api/openai-responses.ts) sends `store=false` and requests encrypted reasoning content for xAI; our current Responses adapter does neither and drops thinking items when constructing history. This is a concrete request/replay gap, not a missing provider-name entry. Complete and qualify it before enabling that batch; no xAI declaration was added in this checkpoint.

- Frozen source, binaries, positive/negative reports and logs: `target/provider-foundations/nvidia-20261121T025927Z-xWHdMa/`. The complete offline parity recipe intentionally exits 1 with **435 pending** (`nvidia-completeness-negative.log`, report `../melchior/target/provider-parity/run-1795229961217157710-84826/`). This confirms the completeness gate still refuses partial coverage; it is not a passing parity result.

### Responses continuation checkpoint (2026-11-21, after NVIDIA)

This advances the Responses/xAI prerequisite identified above; **it does not activate xAI or complete Responses parity**. Provider declarations remain 12/45 and candidate shapes remain 333/1,779. The earlier NVIDIA full-verification receipt describes its own snapshot, not these newer edits.

- All three Responses hostings now send `store=false`; reasoning models request encrypted reasoning even without an explicit effort. The pinned [Responses builder](https://github.com/earendil-works/pi/blob/5a7479a88390d947335a1a6609936897be0a7891/packages/ai/src/api/openai-responses.ts) establishes this request contract. The pinned [shared handler](https://github.com/earendil-works/pi/blob/5a7479a88390d947335a1a6609936897be0a7891/packages/ai/src/api/openai-responses-shared.ts) also identifies Azure's terminal-only encrypted-content case; the new native fixture exercises it across all three hostings.
- Lua retains supported **complete terminal output** in one versioned opaque turn signature. Its ordered items preserve multiple reasoning records, encrypted bytes, message IDs/phases, function-call IDs and original argument strings. Capture is bounded to 256 items/1 MiB; overflow stops with error while retaining measured usage. Normal text and argument deltas still stream. A terminal event is emitted only once.
- Replay requires matching provider, model, actual protocol and resolved endpoint, plus an unchanged visible-text/tool-call projection. Semantic JSON comparison tolerates object-key ordering without treating empty arrays, objects, nulls or large integers as interchangeable. Foreign/malformed/edited state is not replayed. Anthropic, Google and PI Messages explicitly discard this envelope rather than forwarding it as their own signature; Chat Completions already permits only its named reasoning-field signatures.
- The transport resolves the Responses model's effective base URL before request conversion; private scratch origin data never enters HTTP JSON. Model-role wire definitions stay independent and unchanged. This is routing isolation, **not account-identity binding**, which is still required with account/OAuth work.
- Added a separate lossless `melchior.json.decode_value` operation; the existing control decoder is unchanged. Magi's turn aggregation and context reconstruction now preserve signature-only thinking blocks, with no fabricated visible reasoning text. Opaque data remains opaque outside the owning adapter.
- `oslo make test-provider-responses` passes 11 JSON tests, seven continuation tests, 16 protocol regressions and one native test containing six HTTP requests (two per hosting). Tests cover exact replay/order, semantic object-key comparison with original argument-byte preservation, private-field exclusion, hidden reasoning, authentication headers, origin switches, foreign protocols, malformed/oversized state, usage and duplicate terminals. The first native attempt used incorrect fixture event names (`stopped`/`arguments` rather than the existing `stop`/`args`); its failure is retained in `responses-native-first.log`, followed by corrected runs.
- Magi's `test-provider-continuations` passes 15 core-turn and 26 context tests. Its `fmt-check`, all-target release Clippy and all architecture/wire gates pass (`responses-magi-first.log`, `responses-magi-gates.log`). The first Melchior full attempt passed both test suites but failed the no-indexing Clippy policy on two new JSON-object writes; those now use checked object access and insertion, without a lint waiver. The corrected full run (`responses-full-verify-fixed.log`) **passes**, including 887 library tests in each default/all-feature run, integration suites, rustdoc, Clippy, architecture gates, repeated hermetic suite, release build, family-interface/model-role contracts and dependency checks. The release image is dynamic; the existing family-script null-byte substitution warnings remain visible. This is not a fresh whole-family, UI, static-package or live-provider pass.
- Retained receipt: `target/provider-foundations/responses-20261121T032437Z-NyPwAg/`, containing frozen source manifests/archives, failed and successful logs, current parity report and Melchior native images. The parity report still has **16 generic smoke passes, 435 pending and zero live requests**. No additional provider or model was activated and no catalog gap was removed in this checkpoint.
- Next: qualify indexed/interleaved function-call deltas and done-only/incomplete output; bind continuation state across account changes; prove actual selected-family durable journal/resume and retry/cancellation. Then implement the xAI API-key model batch and its separately required subscription authentication. The current complete-terminal envelope is a foundation, not evidence that those cases work.

### Responses indexed-stream checkpoint (2026-11-21)

This closes concrete stream-association and reconstruction defects from the previous checkpoint, without activating providers or changing the independent model wire. Inventory remains 12 declarations, 333 complete candidate shapes and all 1,779 reference records in scope.

- The [OpenAI event reference](https://developers.openai.com/api/reference/resources/responses/streaming-events) identifies output/item and content-part indices and distinguishes argument deltas from final snapshots. PI's [pinned shared handler](https://github.com/earendil-works/pi/blob/5a7479a88390d947335a1a6609936897be0a7891/packages/ai/src/api/openai-responses-shared.ts) tracks output slots. NERV's existing wire has no per-delta call index, so its adapter now serializes indexed slots in source order rather than sending every argument fragment to the most recently opened call.
- The active output slot remains streamed; later items and content parts are bounded buffers. Multiple reasoning channels and refusal text survive. Done events and complete terminal snapshots supply missing suffixes but cannot overwrite received bytes, identities or completed parts. A complete terminal snapshot can reconstruct an otherwise delta-free response. Saved item-done snapshots can supply an omitted terminal output array, retaining exact JSON shapes and opaque state.
- Incomplete max-output responses retain all partial calls and end with `length`; they never produce a continuation signature. Unknown incomplete reasons, contradictory identities/snapshots, orphaned indexed fragments, unsupported indexed tool namespaces and unfinished items in a completed response fail with `error`, preserving final usage. These checks do not add namespace/custom-tool support. Fully unindexed legacy decoding remains a separate compatibility path; it is not evidence for arbitrary mixed or anonymous interleaving.
- Bounds are explicit: 256 output slots, 256 content parts, 1 MiB reconstructed content/identity bytes, 1 MiB saved done-item JSON and the existing 1 MiB final continuation envelope. Done snapshots are held as encoded strings so passing scratch state through the control-value decoder cannot erase empty arrays or nulls. No new Rust wire variant or shared crate was added.
- The initial regression run (`responses-stream-repro.log`) reproduced six failures: wrong-call association, missing done/terminal-only output, incorrect text order, accepted conflicting state and unbounded indices/payloads. A pre-existing protocol fixture supplied only `{type:function_call}` as a terminal call; it now supplies a real identity/name/argument payload and additionally asserts emitted call/argument events. The intermediate fixture failure is retained in `responses-stream-first.log`; no production validation was weakened.
- The current focused recipe passes **11 JSON, 18 continuation/stream, 16 protocol, three client-transport and three native-process tests**. There are **24 native-process HTTP requests** and **15 client-transport HTTP requests** per recipe run, across OpenAI/Azure/Codex hostings. Checks include exact two-tool follow-up bodies, done-only output, partial/conflicting stops, Unicode in single-byte HTTP chunks, retry retraction with identical request bodies, and cancellation followed by a fresh stream. These are fixture requests, not live provider calls.
- The first full `verify` failed after its 901 default library tests passed: the fake-provider example suite passed 31 tests and failed the image corpus decoder (`Stop(Error)` instead of `Stop(EndTurn)`). The old Responses text rounds lacked matching message-added events. The repaired fixture supplies matching message/part added, delta, done and complete snapshots, and its decoder test now asserts concatenated text exactly once. Production orphan checks remain strict. The failed `responses-stream-verify.log` is retained; `responses-stream-fixture-repaired.log` passes all **32 fake-provider tests** and the 51 focused tests. Fresh `responses-stream-verify-repaired.log` records a **full Melchior verify PASS**, including 901 library tests in each default/all-feature run, integration/example suites, Clippy, rustdoc, architecture/hermetic/interface/role and dependency gates.
- The first current-source family image receipt, `../target/acceptance/run-oOF1bUXb/`, passed five adapters but failed all three Responses hostings: its old verifier required a lossy function-call projection without the now-preserved `id` and `status`. Actual requests and journals retained the complete state. That failed receipt remains unchanged. The verifier now requires the full output objects rather than discarding fields, and the scenario adds encrypted reasoning supplied only in the terminal snapshot, with empty summaries, a control character and Unicode opaque bytes.
- The fresh selected-family receipt, **`../target/acceptance/run-eNSRFaBT/`**, passes **eight image adapters, 40 local HTTP requests and 162 mutation refusals** (15 per adapter plus 14 extra for each Responses hosting). OpenAI/Azure/Codex checks prove exact reasoning, call and message items in subsequent HTTP bodies; exact versioned continuation envelopes in Balthasar journals; provider/model/protocol/endpoint binding fields; empty visible thinking; original image bytes after normalization; identical retry requests; and immutable prefixes through same-host session resume. Mutations reject missing/changed opaque state, lost item fields, altered origins, stripped signatures, changed stateless policy and leaked private request fields. Selected running Magi/Melchior/Balthasar images are checked against preserved executable hashes. This is one offline scenario, not whole-family acceptance, authenticated account-isolation proof, host restart/cancellation proof, UI or static-package qualification.
- Frozen source manifests/archives, focused/full logs, failed receipts and selected-family evidence are retained at **`target/provider-foundations/responses-stream-20261121T040410Z-BzzlNz/`**. Magi production source and the independent model wire did not change in this indexed-stream checkpoint. Selected release hashes: Magi `621bec0a52ef1ea31f4df546932b447adc6c96839841bf511fb012d74434c5ea`; Melchior `88ae64b5888482f5f92203f838aa2d4393eaa76f995ea16251b71e26ec0baba0`. These images are dynamically linked; no static claim is made. The earlier whole-family Casper MCP-auth failure remains unresolved.
- Next: account-bound continuation preparation and selected-family restart/cancellation qualification, followed by the xAI API-key batch and its separately required subscription authentication. The client-level cancellation proof and same-host durable resume proof must not be substituted for those remaining cases or full provider qualification.

### Responses credential-binding checkpoint (2026-11-21)

This implements the generic credential-isolation prerequisite. It does not activate a provider, add a login flow or prove a provider's user/account claims. Inventory remains 12 declarations, 333 complete candidate shapes and all 1,779 reference records in scope.

- The initial native regression (`responses-binding-repro.log`) proved that a saved opaque response was replayed after changing the API key. All three Responses hostings now require a matching `origin.credential` digest. It binds authentication configuration, credential identity and effective request headers with domain-separated, length-framed SHA-256. API-key and organization/project-header changes conservatively discard opaque replay. A credential digest is neither a provider account claim nor an authentication of journal contents; it contains no raw token, API key or header value.
- Generic OAuth exchange creates a fresh random local login lineage. Renewal preserves it, including refresh-token rotation and providers omitting a replacement refresh token. Existing token files without a lineage use an access-derived identity that is preserved when promoted on refresh. Login replacement resets the lineage; logout removes its token entry. The additive private token-file field does not change Lua configuration, model cards, the independent model wire or Magi source. Provider-specific Codex/Copilot/xAI account metadata and login details remain separate work.
- Request preparation first checks visible requests without resolving credentials. The resolved binding is a serde-skipped internal option, never accepted from caller JSON. Before any OAuth refresh, the exact bound body is rebuilt and its limits checked; the locked credential is re-read and revalidated, and renewal revalidates again. Actual inference uses that prepared body and the same credential's headers. Retry attempts resolve and bind afresh, so changing accounts while a 503 is being delivered cannot replay the prior account's opaque state on the next attempt. Old envelopes missing a binding fall back to visible history; they are not silently treated as account-qualified.
- `responses-binding-qualified.log` passes **84 focused recipe tests**: three options, one identity digest, one credential binding, 23 OAuth, 11 JSON, 19 Responses continuation/stream, 16 protocol, three transport and seven native-process tests. The targeted Responses native cases use **69 local HTTP requests**, and transport cases use **15**; OAuth control tests make additional local fixture requests. Native coverage includes matching/different keys, organization headers, legacy envelopes, two consecutive OAuth renewals, replacement credentials, logout, a changed credential between attempts, and over-limit bound history producing zero refresh/inference traffic with unchanged credentials. Input-limit and strict-tool recipes and Clippy also pass. Initial compile/fixture adjustments remain in the retained logs.
- Fresh **Melchior full `verify` passes** in `../melchior/target/provider-foundations/responses-binding-verify.log`: **906 library tests** in each default/all-feature run, integration/example suites, Clippy, rustdoc, architecture/hermetic/interface/role and dependency gates. The fresh selected-family image receipt **`../target/acceptance/run-vgC199pP/`** passes **eight adapters, 40 HTTP requests and 168 mutation refusals**. Responses envelopes now require a consistent 64-hex credential binding through journals and resume; added mutations reject changed/missing bindings. Family fixtures use no-auth local endpoints, so authenticated isolation is proved by the separate native tests, not inferred from this family scenario.
- Frozen source manifests/archives, logs and selected-family evidence: **`target/provider-foundations/responses-binding-20261121T042346Z-67fCiq/`**. Selected Melchior release SHA-256: `2a80b90ce13320b5b09d6dbc7cfa9e9268db083c8b77b35a0abb57c74c41e359`; Magi remains `621bec0a52ef1ea31f4df546932b447adc6c96839841bf511fb012d74434c5ea`. Both are dynamically linked. This is not a new whole-family, UI, static-package or live-provider pass; the earlier Casper full-family failure remains unresolved.
- Next: selected-family restart/cancellation qualification and the xAI API-key model batch, with subscription authentication still separate. For that batch, the current [xAI cost reference](https://docs.x.ai/developers/cost-tracking) defines `cost_in_usd_ticks / 10^10` as USD; add measured-cost settlement tests rather than guessing units or treating absent prices as zero. Its [reasoning reference](https://docs.x.ai/developers/model-capabilities/text/reasoning) says Grok 4.5 accepts `xhigh` as an alias of `high`, whereas 4.6+ supports native `xhigh`, and 4.7 supplies encrypted reasoning by default. Review those semantics against the four pinned records before activation; keep the original reference data intact.

### Phase 00 — Freeze the target and establish a baseline

1. Recheck Git status in both repositories. Preserve unrelated changes. Compare relevant paths against the inspected commits:

   ```sh
   git diff --stat e8c5bdd..HEAD -- crates scripts .make.lua ROLES.md
   git -C ../melchior diff --stat 28937f8..HEAD -- src config tests .make.lua
   ```

   Review changed source before using this plan's excerpts. A hash difference is a drift-review trigger, not a reason to discard newer valid work.

2. Pin a PI release/tag and immutable commit. Record the exact 43-row reconciliation in this file and a proposed `../melchior/tests/provider-parity/manifest.json`. The inspected moving `main` and `/latest` documentation are routing aids, not the final specification. If a service has changed, record the discrepancy and get a scope decision rather than silently removing it.
3. Create a proposed `../melchior/tests/provider_parity.rs` harness with isolated HOME/XDG/runtime state and local HTTP fixtures. Reuse `tests/model_refresh.rs`, `examples/fake-provider.rs`, and existing adapter tests rather than introducing a separate test framework.
4. Add proposed `oslo make test-provider-parity` and `oslo make test-provider-parity-complete` recipes in Melchior. The first validates the manifest, runs implemented cases and reports incomplete rows. The second additionally fails on every required TODO/BLOCKED/missing offline case. Do not implement either recipe as a provider-name count or allow an empty test selection to pass.
5. Capture current local `verify` and family results before changing behavior. A historical acceptance receipt is not a baseline for these source revisions. Separate pre-existing failures from new regressions.

**Verify:** both local `oslo make verify` commands and root `oslo make test-family` must produce attributable results. `test-provider-parity` must account for all target IDs; `test-provider-parity-complete` is expected to fail until the implementation phases finish.

### Phase 01 — Correct the extension and readiness foundation

Files: Melchior `provider/endpoint.rs`, `provider/model.rs`, `catalog.rs`, `running.rs`, `wire.rs`; Magi `magi-proto/src/ask.rs`, host catalog/environment/refreshing; targeted tests and wire gates.

1. Separate declaration defaults from resolved models. Preserve explicit per-model `api` and inherit provider `api` only when omitted. Do not merely delete `model.api = api`: direct deserialization currently gives models a default API and must also inherit correctly.
2. Add explicit per-model endpoint overrides and provider/model header configuration where required. Define merge/precedence rules. Resolve credentials only at request time; never serialize resolved headers into cards or receipts. Restrict overrides of signing/auth-owned headers, and do not forward a credential to a changed origin without an explicit configured binding.
3. Retain the typed known-protocol enum for this parity implementation and add required variants deliberately, including Mistral Conversations. Validate declarations against registered/implemented protocols and return attributable diagnostics instead of silently filtering them out. A wholly open custom-protocol type system is a separate design change, not a prerequisite for the reference providers.
4. Introduce a secret-free structured setup/readiness description: credential missing, configuration incomplete, login available, supported auth method, protocol unavailable, or ready. Credential availability is not proof that an account is entitled to a model; remote authorization errors must remain distinguishable.
5. Preserve old wire fields/readers where practical, using additive optional fields and matching types in both independent programs. Document any required family-version change rather than silently changing field meaning.
6. Re-query Melchior after environment/auth changes. Remove Magi's assumption that setting one advertised variable proves readiness; test aliases and all required cloud fields.
7. Ensure a configured OAuth provider can become ready through its existing credential store; do not perform interactive login or cloud token exchanges merely to render a model card.

**Tests:** inherited/overridden APIs through direct deserialization, Lua catalog and actual dispatch; per-model compatibility inheritance; invalid protocol diagnostics; header precedence/redaction; missing/expired/refreshable credentials; API-key aliases; replacement model roles without auth extensions.

**Verify:** Melchior `test-usage`, `test-json-contract`, `test-provider-parity`; Magi `test-model-refresh`, `test-environment`, `gates`. All selected cases pass, including the wire twins check against the sibling checkout.

### Phase 02 — Maintain the catalog and make discovery scale

Files: Melchior `config/providers.lua`, `catalog.rs`, `discovering.rs` and its modules, model/pricing types, discovery tests and proposed catalog-generation scripts/recipes.

1. Maintain a bundled offline catalog with explicit provenance and a reproducible update process. A reviewed generator may consume upstream/provider metadata; normal startup/build/tests must not need a live catalog fetch. Preserve licensing/attribution for imported data or code.
2. Keep unknown pricing distinct from zero-cost pricing. Preserve existing cache-read/write price presence, published limits and capability semantics. Do not advertise tools, vision, reasoning levels or classifier/chat operations from a model-name guess.
3. Make discovery a provider-declared strategy: bundled-only, compatible listing, provider-specific listing, or local service inspection. Supply discovery requests through the same authenticated provider configuration, not a universal bearer-key shortcut.
4. Replace sequential waiting with bounded concurrent refresh, per-provider deadlines, an overall deadline and cancellation. Initial limits should fit within Magi's current 30-second refresh budget; define and test a smaller internal budget with room for IPC and serialization.
5. Preserve last-good data for failed providers and replace it on successful empty responses. Missing credentials should not trigger dozens of futile network calls. Keep offline startup useful with no cache and with stale caches.
6. Key caches by a collision-resistant provider/endpoint/region/account identity. Do not place raw keys or tokens in filenames or cache content. Invalidate or partition account-dependent listings after credential changes; preserve backwards-compatible reads only when safe.
7. Cover credential-restricted catalogs, including the pinned OpenRouter key-aware behavior. A global public listing and an account-authorized listing are not interchangeable.
8. Measure catalog size against ACP's 4,096-model and approximately 1 MiB limits. Add explicit filtering/paging or selection if necessary; never silently truncate usable models or remove the bounds.

**Tests:** many unavailable providers under a fixed deadline, concurrency cap, cancellation, offline startup, stale fallback, successful empty listing, endpoint/account changes, provider-ID normalization collisions, missing price versus explicit free, large-catalog behavior.

**Verify:** Melchior `test-model-refresh`, `test-cache-contract`, `test-usage`, `test-provider-parity`; Magi `test-model-refresh`, `test-acp`. Refresh fixtures must prove bounded wall time, not only eventual completion.

### Phase 03 — Add compatible API-key providers in qualified batches

Together adds 16 explicitly selected records to the phase-table batches above, and
Hugging Face adds 87 more. Both batches pass focused/native-host tests and full local
verification. Ant Ling adds three records with focused/native-host, retained debug/release
and full local verification passing. Complete provider qualification remains open.

**Catalog import design (implemented; broader parity remains in progress):** keep the keyed `melchior.provider(id, table)` registrar. A provider explicitly selects pinned records with `catalog = { source = "pi-1.6.0", provider = "groq", models = { "chat:llama-3.3-70b-versatile" } }`. No wildcard or implicit all-model import is allowed. Selection refuses missing, duplicate, non-chat or pending-mapping records atomically, before discovery or credentials. Local `models` entries overlay matching imported models at top-level field boundaries; nested values replace rather than deep-merge, and distinct entries append. Existing declarations without `catalog` remain unchanged. Containing-provider identity, explicit per-model APIs and credential-origin validation are preserved. The generated report remains candidate data; Groq/Cerebras/NVIDIA activate four explicitly selected records, xAI four, Meta seven, MiniMax global/China four and Xiaomi PAYG/token plans nine more, not all candidates.

Files: primarily Melchior provider declarations, model catalog and compatibility settings; add adapter changes only where fixtures demonstrate a real dialect difference.

1. Begin with a small batch such as Groq, Cerebras, NVIDIA and xAI after confirming their pinned protocol mappings.
2. Continue through every K-group matrix row, including regional/plan variants. Preserve separate IDs, credentials and endpoint binding even where one implementation is reused.
3. Register reviewed static catalogs where a service cannot publish suitable metadata. Do not set `discover = true` indiscriminately.
4. For each provider, test the actual serialized request, response conversion and errors using representative models. Include tool results, schema differences, reasoning behavior, usage and cache fields, and unsupported options.
5. Run the common parity harness for every alias/region as well as every protocol family. Shared implementation coverage does not prove a preset has the correct endpoint or credential variable.
6. Update the matrix with implementation and offline evidence, leaving live status UNVERIFIED until an authorized request succeeds.

**Verify after each batch:** Melchior `test-provider-parity`, `test-streaming`, `test-usage`, `test-overflow`, `test-cache-contract`; Magi `test-model-refresh`. All pre-existing provider fixtures remain green.

### Phase 04 — Complete authentication and expose it safely

Files: Melchior `provider/endpoint.rs`, `provider/oauth/`, `provider/client.rs`, `signing.rs`, CLI/wire dispatch; Magi model/auth commands and presentation, host refresh, protocol tests.

**Implemented foundation:** `:login` and Melchior `auth --stdio` manage one active saved
credential per provider, with environment-first resolution, configuration binding,
masked input, confirmed removal, PKCE callback validation and cancel/reap semantics.
Local auth replies stay outside transcript/observer storage; successful changes refresh
model readiness. Both repositories expose `oslo make test-login`. This does not complete
the remaining provider-specific, secret-manager or multi-account-profile work below.

**Copilot addition (2026-11-21):** Melchior now owns GitHub device authorization,
bounded pending/slow-down/rate-limit polling, Copilot token exchange and renewal,
subscription endpoint validation, account-filtered models, and protocol-specific
Bearer headers. Magi's existing float displays a neutral device reply with open,
copy-code/link and cancel actions. No tool execution/MCP or memory implementation
was added. Logout fences pending login writes, and renewal preserves the existing
credential-bound Responses preflight contract. See [LOGIN.md](LOGIN.md) for usage.

The bundled selection adds 14 complete pinned candidates and retains the legacy
Sonnet 4.5 entry. Pending GPT/Claude compatibility mappings and the System One
classifier are not silently enabled. Unlike PI's reference, login never changes
GitHub model policies automatically. The explicit `COPILOT_GITHUB_TOKEN` override
remains a direct bearer path; generic `GITHUB_TOKEN` is no longer treated as a
Copilot access token. Expiring custom GitHub OAuth registrations and a bundled
Enterprise Server domain flow remain unsupported. Real account qualification is
not claimed by the localhost fixtures.

**Qualified checkpoint:** Melchior `oslo make verify` passes; its focused Copilot
recipe passes 18 cases, including 8 native Copilot tests against the final selected
image. Magi's login recipe passes 27 cases, with actual device-code cancellation
and process-identity checks. Canonical family receipt
`../target/family/run-r8kbTAER/summary.json` records **6,979 reported passes,
16 ignored, zero failures/leaks**; all 45 focused cases were confirmed present.
Evidence and actual-renderer previews are in
`target/copilot-verification/run-20261121-BR5K0WjQ/`.
Casper and Balthasar remain unchanged. This is local fixture qualification, not
live GitHub authorization or complete Copilot catalog parity.

1. Introduce explicit auth strategies without discarding the existing OAuth persistence and cross-process locking. Resolve the full pinned provider matrix: environment keys, stored keys where supported, browser PKCE, device authorization, provider-specific token exchange, cloud credentials and no-auth local endpoints.
2. Define credential precedence across explicit user configuration, stored credentials and environment variables. Test it. Logout removes the selected stored credential; it must not claim to clear an environment variable or revoke a provider-side token unless it actually does so.
3. Support trusted secret-manager commands through an explicit owner-configured argument vector, timeout/output bounds and scrubbed child environment. Do not execute arbitrary project/model-supplied shell text or adopt an implicit `!command` parser throughout configuration. Document this syntax difference from PI rather than weakening NERV's trust boundary.
4. Add provider-specific login details only from supported/documented integrations, with appropriate public-client registration. Do not extract credentials from other applications or assume one provider's OAuth flow works for another.
5. Cover browser and headless operation, callback state/PKCE, device polling intervals and expiry, cancellation, concurrent refresh, rotated refresh tokens, denied consent and atomic failure-safe persistence.
6. Expose secret-free auth methods/actions over an optional model-role extension. Magi presents URL/device-code instructions, masked entry when appropriate, progress and cancellation; Melchior handles the credential lifecycle. Keep standalone headless Melchior usable.
7. Refresh readiness and account-filtered models after login/logout/credential changes. Synchronize attached Magi screens without publishing credentials in cards, ordinary events, snapshots or journals.
8. Complete Codex and Copilot against pinned provider behavior, including required account metadata, endpoint selection and model-level protocol choices. Preserve existing API-token paths where valid.
9. Make reasoning controls use supported model capabilities rather than only a fixed list plus a boolean. If adding levels such as `xhigh`, update both wire sides and saved-session compatibility explicitly.

**Tests:** disposable credential stores; mocked authorization/token services; stale token and concurrent rotation; device cancellation; interrupted save; logout during refresh; secret redaction; no implicit login on model listing; optional auth extension absent; headless flow; account switching.

**Verify:** Melchior `test-oauth-storage`, `test-streaming`, `test-provider-parity`; Magi `test-environment`, `test-model-refresh`, `test-acp`, `gates`. Add named auth fixture tests to the parity recipe so storage tests alone cannot satisfy this phase.

### Phase 05 — Finish Azure and Vertex

1. Azure: normalize resource/base URLs, map logical models to deployment names, make API-version behavior explicit, and preserve per-model Responses versus Chat routing. Do not retain the hardcoded preview URL as universal Azure support.
2. Vertex: reuse existing Google conversion; implement supported API-key and ADC modes, project/location configuration, short-lived token refresh and documented credential sources. Treat API-key mode and bearer-token mode as different request configurations.
3. Keep cloud auth in native, bounded code. Prefer maintained provider libraries where appropriate, but review their dependency, TLS, runtime and static-build impact before adoption.
4. Test global/regional endpoints, absent setup fields, inaccessible deployments, expired tokens and denied permissions. Do not label every setup failure as a missing API key.

**Verify:** Melchior `test-provider-parity`, `test-streaming`, `test-usage`, `test-overflow`, `verify`; Magi `test-model-refresh`, `test-acp`. Cloud credential tests use isolated fake sources, never ambient production credentials.

### Phase 06 — Implement Bedrock and Mistral Conversations

**Bedrock:**

1. Implement the pinned AWS credential sources, region/profile handling, session credentials and supported bearer mode. Do not make static environment keys the only working path while claiming credential-chain parity.
2. Introduce a reviewed transport/framing seam: existing SSE and whole-JSON paths remain intact; Bedrock receives bounded binary AWS EventStream decoding or an SDK decoder that satisfies equivalent limits.
3. Validate lengths, checksums/framing where applicable, headers, truncated records, error records and arbitrary byte partitions. Cancellation must interrupt reads and credential work.
4. Serialize Converse messages, tools, images, reasoning/cache fields and model/inference-profile identifiers correctly. Sign the exact final URL/headers/body bytes; changing a request after signing is forbidden.
5. Translate events to the existing normalized stream and accounting contract. Do not add a parallel retry or spending authority through an SDK's hidden retries; disable or explicitly integrate them and account for every attempt.

**Mistral:**

1. Add the pinned `mistral-conversations` dialect, enum/registration, model mapping and fixtures. Despite its protocol name, PI's pinned implementation calls the native **`/v1/chat/completions`** endpoint, not the separate managed Conversations API. See the [pinned adapter](https://github.com/earendil-works/pi/blob/5a7479a88390d947335a1a6609936897be0a7891/packages/ai/src/api/mistral-conversations.ts) and [Mistral's native/OpenAI endpoint distinction](https://docs.mistral.ai/inference/conversations/chat-completion).
2. Reuse bounded HTTP/SSE infrastructure. Translate native thinking/content chunks, nine-character call IDs and paired results, images, model-specific reasoning controls, cache usage and finish reasons. Keep execution and MCP in Casper; do not use Mistral-hosted tools/connectors or add an agent loop.
3. Do not alias the generic OpenAI adapter or use `/openai/v1/chat/completions` and call that pinned parity. Preserve scoped cache/session identity and strict-tool opt-in. Qualify the 34 pinned Mistral records offline before advertising the batch as tested; live account/model availability remains separate.

**Verify:** Melchior `test-provider-parity`, `test-streaming`, `test-usage`, `test-overflow`, `test-cache-contract`, `verify`. Binary decoder tests must exercise split records, corruption, oversized lengths and cancellation. Run the existing release/static build workflow before accepting cloud dependencies.

### Phase 07 — Finish special services and qualify existing providers

1. Cloudflare Workers AI: account-aware endpoint construction, token setup, advertised chat capabilities and applicable classifier handling.
2. Cloudflare AI Gateway: distinguish gateway authentication from upstream credentials and billing mode; do not forward credentials between unrelated origins.
3. Radius: verify whether the existing Pi Messages adapter matches the pinned service contract; implement supported auth and dynamic catalog with endpoint-specific offline state. Record unavailable/private access as BLOCKED, not unsupported by omission or falsely verified.
4. TypeSafe: provide direct classifier access using the existing Decisions/decider abstraction where its contract genuinely matches. Preserve schemas, typed results and failure behavior; do not place a classifier into the chat picker as a text-generating model.
5. Complete gateway model-level routing/header requirements for Vercel, OpenCode and other applicable rows.
6. Requalify every R-group provider and both retained extras. Google call identity, exact arguments, ordered signed replay, thought-token accounting and discrete controls have native and actual-host proof, with full native/Magi and retained debug/release checks passing. Broader catalog/protocol qualification and complete CLI/memory restart remain open.
7. Verify cross-provider history normalization: preserve opaque signatures only where valid, maintain tool/result pairing, and avoid leaking one provider's private reasoning/continuation metadata into another protocol.

**Verify:** Melchior `test-provider-parity-complete`, `test-json-contract`, `test-cache-contract`, `test-usage`, `verify`; Magi `test-model-refresh`, `test-acp`, `verify`. The complete offline gate must cover all reference rows and retained extras, with no required cases skipped.

### Phase 08 — Prove end-to-end support and preserve evidence

1. Build selected family binaries through the native recipes. Record repository revisions, dirty-diff identity, binary hashes and actual executed image identity. Do not validate an older installed binary accidentally.
2. Run both local verification suites, full root family verification and deterministic acceptance. Preserve failing receipts and explicit skips; do not turn a reduced suite into the final gate.
3. Exercise real Magi model selection, auth state transitions, refresh, multi-screen updates, cancellation and session resume with the new catalog. Include the actual terminal and ACP/editor surfaces affected by the change, with captured evidence.
4. Verify static/release packaging through the existing Nix/release workflow after dependency changes. Source tests alone do not prove the shipped artifact works.
5. Extend the existing cost-capped live acceptance runner where necessary for each protocol/auth method. The current runner's success for one provider does not certify all new adapters.
6. With explicit user authorization, use synthetic conversations and approved accounts to perform small live tests for every target integration. Include text/classification as appropriate, a tool loop where supported, usage, and applicable login/refresh behavior. Never upload the user's source tree as a smoke test.
7. Report exact provider/auth/model combinations tested. Missing credentials or unavailable provider access leave corresponding live rows BLOCKED/UNVERIFIED and prevent an unqualified full-parity claim.
8. Document setup, environment aliases, precedence, unsupported capabilities, offline behavior and recovery. Update this plan's phase/status tables and retain receipt locations.

**Verify:** all commands in Section 5 applicable to the completed implementation exit zero, the offline completeness gate passes, and the live matrix contains real receipts for every claimed supported combination. No required test is silently skipped.

## 5. Verification commands

The repositories use Oslo `.make.lua` recipes. Use `oslo make`, not ad hoc Cargo commands or a newly invented GNU Makefile. From this document's directory:

| Purpose | Command | Expected result |
|---|---|---|
| Melchior full local verification | `(cd ../melchior && oslo make verify)` | Exit 0; all declared local gates executed |
| Provider byte/HTTP boundaries | `(cd ../melchior && oslo make test-streaming)` | Exit 0; streaming and provider fixtures pass |
| Pinned catalog input audit | `(cd ../melchior && oslo make test-provider-catalog)` | Exit 0; all source hashes/counts and operation identities match the reviewed snapshot |
| Bounded authenticated discovery | `(cd ../melchior && oslo make test-provider-discovery)` | Exit 0; cache identities, deadlines, cancellation and real-process discovery tests pass |
| OAuth storage/locking | `(cd ../melchior && oslo make test-oauth-storage)` | Exit 0; isolated persistence fixtures pass |
| Usage/pricing/spending | `(cd ../melchior && oslo make test-usage)` | Exit 0; existing and added accounting cases pass |
| Overflow classification | `(cd ../melchior && oslo make test-overflow)` | Exit 0; bounded recovery and refusal cases pass |
| Cache serialization/lifecycle | `(cd ../melchior && oslo make test-cache-contract)` | Exit 0; no cache/accounting regressions |
| Lua/JSON fidelity | `(cd ../melchior && oslo make test-json-contract)` | Exit 0; typed data survives conversion |
| Provider discovery | `(cd ../melchior && oslo make test-model-refresh)` | Exit 0; refresh/fallback cases pass |
| Magi environment propagation | `oslo make test-environment` | Exit 0; readiness/credential-overlay cases pass |
| Magi model refresh | `oslo make test-model-refresh` | Exit 0; refreshed choices reach clients |
| Optional model display labels | `oslo make test-provider-labels` | Exit 0; JSON/CBOR compatibility, unchanged routing, picker rendering, ACP identity and native refresh/snapshot propagation |
| ACP contracts and bounds | `oslo make test-acp` | Exit 0; model settings and transport cases pass |
| Magi architecture/wire gates | `oslo make gates` | Exit 0; twins checked with sibling present |
| Magi full local verification | `oslo make verify` | Exit 0; all declared local gates executed |
| Selected family build | `(cd .. && oslo make build)` | Exit 0; selected artifacts identified |
| Family integration | `(cd .. && oslo make test-family)` | Exit 0; required selected-image cases pass |
| Full family verification | `(cd .. && oslo make verify)` | Exit 0; required member/family gates pass |
| Deterministic acceptance | `(cd .. && oslo make acceptance)` | Exit 0; required scenarios pass |

**Parity recipes added during implementation, absent from the initial inspected commits:**

| Purpose | Command | Expected result |
|---|---|---|
| Incremental provider qualification | `(cd ../melchior && oslo make test-provider-parity)` | Manifest validated; implemented cases pass; all unfinished rows explicitly reported |
| Complete offline parity | `(cd ../melchior && oslo make test-provider-parity-complete)` | Every required reference/extra row has passing offline cases; no TODO/BLOCKED/skipped required cases |
| Isolated image worker | `oslo make test-provider-image-worker` | Pixel, resource, cancellation and strict worker-protocol checks pass |
| Native image-history integration | `oslo make test-provider-image-history` | Pre-admission normalization, immutable history, failures and resume checks pass with selected peer prerequisites |
| Selected-family image transport | `(cd .. && oslo make test-provider-images-family)` | All eight implemented image adapters pass HTTP retry/tool/resume contracts and corrupted-capture refusals; selected running roles verified |

Extend relevant existing recipes to include newly added integration tests; do not assume a Rust test file is exercised by a filtered recipe. Add an inventory assertion so misspelled filters or zero executed cases fail.

Live acceptance uses the existing root `oslo make acceptance-live` recipe with explicit `NERV_LIVE_PROVIDER`, `NERV_LIVE_MODEL`, `NERV_LIVE_CAP_USD` and `NERV_LIVE_SEND=yes` inputs. Do not set these or invoke live acceptance without approval. Adapt the runner first if it cannot exercise a target protocol safely. No live command should be a dependency of the default offline suite.

If a gate requires unavailable tools, credentials or environment access, report that exact limitation. Do not replace `verify`, selected-image checks or UI acceptance with a convenient subset.

## 6. Required test matrix and fixtures

Use existing fixtures as structural examples:

- `../melchior/tests/model_refresh.rs`: isolated CLI/catalog requests and local HTTP provider.
- `../melchior/src/mind/lua/adapter/protocols.rs`: protocol conversion fixtures.
- `../melchior/src/mind/lua/adapter/fidelity_tests.rs`: structured JSON/content fidelity.
- `../melchior/src/mind/provider/client/tests.rs` and `sse/partitions.rs`: HTTP/byte boundaries.
- `../melchior/src/mind/provider/oauth/persistence.rs`: private storage and concurrency.
- `crates/magi-host/tests/model_refresh.rs`: refreshed models reaching attached clients.
- `scripts/gate-twins.sh`: deliberately duplicated Magi/Melchior wire shapes.

Add named cases for:

1. Catalog identity: every target ID, correct endpoint and credential aliases, model-level overrides, no accidental duplicates or silent invalid-entry loss.
2. Authentication: absent, malformed, expired, refreshed, rotated, revoked and denied credentials; simultaneous processes; logout; cancellation; provider/account binding.
3. Text: ordinary, empty, Unicode-split, truncated and malformed responses; exactly one terminal outcome.
4. Tools: partial JSON arguments, parallel calls, repeated tool names, results in the next turn, schema restrictions and unsupported-tool refusal.
5. Images: input and tool-result images for advertised models; existing explicit fallback for non-vision models.
6. Reasoning: supported levels, disabled/unsupported levels, opaque signatures, redacted content and valid provider switching.
7. Usage: input/output/cache/reasoning semantics, cumulative versus incremental reports, absent versus zero rates, failed/retried attempts and spend-cap enforcement.
8. Errors: authentication, throttling, overload, transport loss, context overflow, invalid requests and provider-specific bodies; no broad error-to-overflow rewriting.
9. Discovery: offline startup, cache migration, account/endpoint changes, successful empty catalogs, partial failures, bounded concurrency/time/size and cancellation.
10. Integration: headless Melchior, Magi startup/selection/login/refresh, child sessions, resume, ACP and multiple attached clients.

A receipt records provider ID, auth method, model/operation, capability cases, source revision, selected executable identity, fixture/live classification, result and timestamp. Live receipts also record the approved spending cap and observed usage where available. Never store raw credentials, auth headers, full private provider responses or real user prompts as evidence.

## 7. Completion criteria

All must hold before claiming full provider parity:

- [ ] PI reference is pinned to an immutable revision; all reference IDs and auth methods are accounted for.
- [ ] The seven existing matching IDs and both extra local IDs retain configuration/session compatibility.
- [ ] Per-model routing/compatibility overrides survive configuration and reach actual dispatch.
- [ ] Invalid declarations produce actionable diagnostics; unsupported protocols cannot appear ready.
- [ ] OAuth/cloud readiness is authoritative, secret-free and correctly displayed by Magi.
- [ ] Every required provider has reviewed model metadata and a valid static or dynamic discovery strategy.
- [ ] Bedrock, Mistral, Azure, Vertex, subscription providers and specialized services have their real required behavior, not placeholder entries or proxy-only substitutes.
- [ ] Default tests are isolated and offline; all required offline parity cases pass with no silent skips.
- [ ] Full local/family, affected UI/ACP and static/release acceptance evidence is retained.
- [ ] Every live support claim has a matching authorized receipt. Any remaining blocked rows are explicitly listed and overall completion remains partial.
- [ ] No credentials appear in ordinary IPC model metadata, snapshots, journals, logs or receipts.
- [ ] No excluded website/root-doc/tool-policy changes are present.
- [ ] Documentation and this status ledger reflect exactly what is implemented and verified.

## 8. Escalation and stop conditions

Stop the affected workstream and report rather than improvising when:

- Required writes fall outside the authorized workspace or require excluded files.
- The pinned reference requires an unavailable/private service, unsupported third-party OAuth client, forbidden credential extraction, or account access not provided by the user.
- An API/protocol mismatch invalidates the planned adapter reuse. Update the design and tests; do not hide it behind a compatibility flag without evidence.
- The solution would weaken containment, secret binding, stream limits, spending limits, cancellation or selected-image verification.
- A dependency breaks independent builds or the static release target; obtain a reviewed dependency/transport decision before changing distribution policy.
- A reproducible regression remains unresolved after investigation. Preserve failing evidence and narrow the cause; do not delete or relax the assertion to finish the phase.

Routine authorized test-fixture repairs are part of implementation and do not require repeated approval. Missing credentials block live acceptance, not unrelated offline work. Do not mark a phase complete merely because no further progress is possible with available accounts.

## 9. Git and maintenance rules

- Use built-in edit/patch tools. Do not use Python to edit files.
- Respect each repository's own Git boundary; keep source changes and family integration changes attributable.
- Do not commit or push unless requested. If commits are authorized, use a Conventional Commit title only, no body, signatures, sign-off or co-author trailers; keep the message within the user's 50-character limit. Example: `feat(providers):add groq support`.
- Keep provider/catalog updates separable from auth/transport refactors so regressions can be isolated.
- Refresh the pinned parity baseline deliberately; upstream additions must not silently expand or shrink the accepted scope during implementation.
- Model metadata, endpoint versions, OAuth integrations and account-filtered catalogs need ongoing review. A green historical fixture does not prove an external service still behaves the same way.
- Do not use OpenRouter availability as proof of direct provider support. Preserve proxy/custom endpoints as options, not substitutes for the named integration.

## 10. Reference sources

Source links below are pinned where applicable. `/latest` documentation remains supporting guidance, not the immutable parity contract.

- [PI providers documentation](https://docs.pi.dev/latest/providers) — authentication and provider-specific setup.
- [PI provider registry](https://github.com/earendil-works/pi/blob/5a7479a88390d947335a1a6609936897be0a7891/packages/ai/src/providers/all.ts) — the 43-provider reference inventory.
- [PI API/provider types](https://github.com/earendil-works/pi/blob/5a7479a88390d947335a1a6609936897be0a7891/packages/ai/src/types.ts) — separate provider identities, protocols and operations.
- [PI transcript normalization](https://github.com/earendil-works/pi/blob/5a7479a88390d947335a1a6609936897be0a7891/packages/ai/src/utils/transcript.ts) — ordered instruction and tool-state replay.
- [Kimi dynamic tools](https://platform.kimi.ai/docs/guide/use-dynamic-tool-loading) — complete inline declarations, position and replay requirements.
- [PI custom providers](https://docs.pi.dev/latest/custom-provider) — reusable adapters and provider integration contracts.
- [PI models configuration](https://docs.pi.dev/latest/models-configuration) — provider/model overrides and compatibility configuration.
- [PI Copilot provider](https://github.com/earendil-works/pi/blob/5a7479a88390d947335a1a6609936897be0a7891/packages/ai/src/providers/github-copilot.ts) — mixed APIs and credential-dependent catalog filtering.
- [PI Mistral provider](https://github.com/earendil-works/pi/blob/5a7479a88390d947335a1a6609936897be0a7891/packages/ai/src/providers/mistral.ts) — Conversations rather than generic Chat-only coverage.
- [Amazon Bedrock ConverseStream](https://docs.aws.amazon.com/bedrock/latest/APIReference/API_runtime_ConverseStream.html) — native inference request/event contract.

## Key Learnings:

1. The existing provider/adapter split is suitable for broad coverage; the expansion belongs primarily in Melchior.
2. Registry correctness, authentication readiness and bounded discovery must be fixed before catalog breadth can represent reliable support.
3. Catalog entries, offline fixture qualification and live provider acceptance are separate milestones and must remain separately reported.
