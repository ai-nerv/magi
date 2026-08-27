# axum — What Is Missing

> Companion to [`PLAN.md`](PLAN.md). Not tracked by git.
> Reference checkouts: `xtra/pi/` (primary), `xtra/tau/` (secondary).
> `PLAN.md` says what was built and why. This says what was not, and what it costs.

---

## 0. What this is

Every subsystem of axum was put beside its counterpart in Pi and, where Pi has no equivalent,
in Tau. Seven parallel comparisons — the turn loop, the provider layer, the tool layer, the
prompt and context layer, the session and config layer, the terminal, and the peer protocol —
each producing a list of things the reference does that axum does not.

Then every claim was attacked. A second pass took each gap and tried to refute it: find the
code that closes it, find the config that supplies it, find the peer that could do it today.
**A gap only survived if both halves had a file and a line: the reference implementation, and
the axum code that proves the absence.** Claims that could only be supported by "grep found
nothing" in one direction were dropped.

**77 findings survived.** They merge to **65 distinct gaps**, because seven surveys running in
parallel found the same wound from different angles — unbounded tool output was reported four
times, images four times, compaction's cut point twice, `Retry-After` three times. That
convergence is itself evidence: a defect that shows up in four unrelated subsystem reviews is
not a subsystem's problem.

**Six of the 65 are choices**, already written down in `PLAN.md` §9 or in M10+'s "only on
demand" line. They are in §8 and they are not to be built. That leaves **59 gaps that are
debt**: 7 critical, 20 high, 28 medium, 4 low.

This document does not schedule anything. It is the record of what the difference actually is,
so that the next milestone is chosen against evidence rather than against interest.

---

## 1. The pattern, again

`PLAN.md`'s closing sweep — *for every declared field, variant and constant, find the code that
produces it and the code that consumes it* — has paid off in every milestone since M4. It was
run again here, and it still has not been exhausted:

| declared | consumed | produced |
|---|---|---|
| `ToolReport::Progress` — "output so far, for a tool worth watching" (`axum-proto/src/lib.rs:605`) | `axum-tools/src/process.rs:297`, into a local `String` | **by nothing.** `axum-cli/src/shell.rs` never sends one |
| `Content::Image` (`axum-model/src/lib.rs:81`) | three Lua adapters; `compact.rs:50` measures it | **by nothing** — already recorded in `PLAN.md`'s Next action |
| `axum.theme = "dark"` (`axum-lua/src/lib.rs:16`) | **by nothing** — `config.rs` reads only `trusted`/`system`/`thinking`/`model` | the doc comment |
| `axum.keys = {…}` (harvests fine, `axum-lua/src/tests.rs:31`) | **by nothing** in `axum-cli` | a config that sets it |
| `Usage::price(Cost)` (`axum-model/src/usage.rs:45`) | **by nothing** outside its own unit test at `usage.rs:135` | every turn, journalled |
| the Lua client stub `axum.lua` (`axum-lua/src/stub.rs:15`) | callers told to run `axum lua-api` (`lua/axum.lua:9`) | **no such subcommand** (`main.rs:64-101`) |
| `accepts_images()` (`axum-provider/src/model.rs:139`) | the catalog reports it | nothing that could send one |

And three items `PLAN.md` §8 lists as **stolen wholesale** are declared and absent:

- **"parallel tool execution with sequential preparation and source-order emission"** — `turn.rs:398`
  is a plain `for` loop, and `registry.rs:11-13` documents the trait as "deliberately not
  `Send + Sync`". Not started, and architecturally closed as written.
- **"shared account cooldown"** — `client.rs:164` computes backoff per call with no state
  between calls. The deterministic jitter beside it in the same §8 line *was* built (`retry.rs:91`).
- **"`transform_messages()` as a mandatory inbound pass"** — M5 fixed tool-call replay, which is
  what that line meant at the time. There is still no inbound repair pass, which is why §2.2
  can produce a request the provider rejects outright.

---

## 2. The areas, ranked

```text
  D1  THE LOOP                                     3 crit · 4 high · 3 med
      output nothing bounds · a cut that orphans a tool result ·
      no hooks · no steering · no interception · tools run one at a time

  D2  THE WIRE                              2 crit · 6 high · 5 med · 1 low
      no prompt caching · nothing streams · Retry-After discarded ·
      no sampling params · no JSON repair · sign-in signs in to nothing

  D3  THE SESSION                                  1 crit · 3 high · 7 med
      you can reach exactly one session · rewind is not a tree ·
      no search · no export · no scripted mode · cost is computed, never shown

  D4  THE PEERS                                            1 crit · 2 high
      the shell hands its own control pipe to the command ·
      a peer can answer a call and observe nothing · a peer has nowhere to put state

  D5  WHAT THE MODEL IS TOLD                        2 high · 1 med · 1 low
      no skills · no saved prompts · a peer cannot ship its own instructions ·
      the prompt names tools whether or not they exist

  D6  THE SCREEN                                   2 high · 10 med · 2 low
      code is one colour · six commands · one palette · no rebinding ·
      no search · no mouse · no clipboard · no `!`

  D7  THE TOOLS                                             1 high · 2 med
      read cannot page and has no byte cap · edit does one site at a time ·
      edit cannot match a CRLF file at all
```

---

## 3. D1 — the loop

### 3.1 Tool output is unbounded, end to end — CRITICAL

*Found independently by the tools, agent-loop, prompt-context and Tau surveys.*

**Pi:** `coding-agent/src/core/tools/truncate.ts:11-12` caps every tool result at 2000 lines /
50KB; `bash.ts:437-452` appends `[Showing lines X-Y of N. Full output: <path>]`;
`agent/src/harness/utils/shell-output.ts:51-78` streams to the full-output file while keeping
only the tail in memory. **Tau:** `tau-ext-shell/src/shell_output_spool.rs:10-12` spools to an
owner-locked file capped at 16 MiB, expiring by age and by call count.

**axum:** there is no cap at any point on the path. `axum-cli/src/shell.rs:225-247` accumulates
every line into one `String`; `axum-tools/src/process.rs:283-307` concatenates every `Progress`
chunk plus the final output; `axum-tools/src/lib.rs:27-35` `Output` is a bare `String` with no
size field; `registry.rs:110-127` returns it verbatim; `turn.rs:398-418` journals it whole. The
only byte caps in the tree are `process.rs:51 COMPLAINT_LIMIT = 4096` (a peer's *stderr*),
`system.rs:26 PROJECT_LIMIT` (AGENTS.md), and `axum-ipc/src/codec.rs:14 MAX_FRAME_BYTES = 16 MiB`,
which kills the connection rather than truncating.

**What it costs:** one `cat package-lock.json`, one `git log -p`, one verbose build. The result
is appended to the journal, so it is replayed on every subsequent request; it sits inside the
`KEEP = 8` tail that `compact.rs:27` preserves verbatim, so compaction cannot remove it; and
`compact.rs:88-105` then feeds that same blob to the summariser. A single noisy command costs
the whole conversation, and the failure gives the model nothing to act on.

**Shape of the fix:** one place, in Rust, at the chokepoint — `Registry::call` or `turn.rs`
before the journal write. Not in the peers: a peer cannot be trusted to cap itself, `config/tools/bash.lua`
has no knob to set, and in-daemon Lua tools cannot write a spill file (`axum-lua/src/fs.rs`
offers `fs.ls` and nothing else). The spill path is `Output`'s missing second field.

### 3.2 Compaction cuts between a tool call and its result — CRITICAL

*Found by the agent-loop and prompt-context surveys; the second reproduced it.*

**Pi:** `agent/src/harness/compaction/compaction.ts:312-345` `findValidCutPoints` — `case "toolResult": break`,
a tool result is never a cut point; `:346-360` snaps back to the turn start; `:390-410`
`findCutPoint` moves the cut to the nearest valid boundary.

**axum:** `compact.rs:77-81` `covers(entries)` is `(entries > KEEP + 1).then(|| entries - KEEP)` —
arithmetic over a `usize` that never sees an `Entry` and cannot know its kind. `turn.rs:105`
writes that index verbatim as `Entry::Compaction { replaces }`. `context.rs:151` retains
`i >= *replaces`. When the surviving head is an `Entry::Tool` whose `Entry::Assistant` was
compacted away, `context.rs:94` correctly drops the `ToolCall` — and `context.rs:101-115` pushes
the `ToolResult` unconditionally. `config/apis/anthropic-messages.lua:45-46` then emits
`{type="tool_result", tool_use_id=…}` with no preceding `tool_use`.

**What it costs:** a tool round journals one Assistant entry followed by N Tool entries, so the
boundary lands on a Tool entry routinely. Anthropic returns 400. `retry.rs:40` classifies 400 as
`RetryClass::Invalid` — not retryable, not `Overflow`, so the compact-and-retry path never
fires. The `Compaction` entry is append-only and replayed on every rebuild, so the session is
dead and `/clear` is the only exit. `context.rs:222` already names this shape as the one
"Anthropic rejects outright".

**Shape of the fix:** `covers` takes the entries, not a count, and walks back to the nearest
index that is not a `Tool`. Same file, same pass — `context.rs` already answers "which entries
are live" in one place (`PLAN.md` §5d), and this belongs in that pass rather than as a second
rule that has to agree with it.

### 3.3 No lifecycle hooks — nothing user-supplied runs inside a turn — CRITICAL

*Found by the agent-loop and sessions-config surveys.*

**Pi:** `coding-agent/src/core/extensions/types.ts:1239-1281` is the `on(event, handler)` surface
— roughly 45 named points; `runner.ts:984` `emitContext` lets a handler rewrite the message
list, `:1016` rewrites the provider request, `:1050` the headers.

**axum:** `axum-lua/src/engine.rs:264` — `const REGISTRARS: &[&str] = &["provider", "agent", "shell", "mux"]`,
plus `axum.api` (`:205`) and `axum.tool` (`:220`). `engine.rs:238-256` `harvest` treats every
other assigned field as inert data. `turn.rs:292-440` calls nothing user-supplied at any point.

**What it costs:** everything downstream. §3.4, §3.5, §5.3 and §6.x are all the same absence
seen from different sides. A user cannot inject per-turn context, audit tool activity, block a
mutation, rewrite a header for a corporate proxy, or react to a session starting.

**Note before anyone builds it.** `engine.rs:261-263` states the exclusion deliberately:
registrars are *"named for the thing being described, never for when it happens."* A hook is
named for when it happens. Adding one reopens a settled decision, and it should be reopened on
purpose or not at all. The cheaper move that does not reopen it is §3.5 — a single declarative
policy object rather than a general event bus.

### 3.4 Nothing can be said to a running turn — HIGH

**Pi:** `agent/src/agent.ts:231-232` steering and follow-up queues; `agent-loop.ts:182-190`
drains pending messages into context before the next assistant response; `:263-268` follow-ups
restart the outer loop.

**axum:** `axum-cli/src/keys.rs:63-64` says it outright — *"a prompt sent mid-turn would be a
steering message, which is an M2 concern, so for now Enter during a turn does nothing."*
`driver.rs:120` passes `app.is_busy()`; `keys.rs:205` swallows Enter. `lib.rs:158-161` has no
busy-aware `SubmitPrompt` path and `worker.rs:101-111` queues a whole second turn behind the
first, deliberately.

**What it costs:** the only correction available is Interrupt, which throws the turn away. A
user who sees the model start down the wrong path at round 3 waits out up to `MAX_ROUNDS = 24`
(`turn.rs:285`) or pays for an abort.

### 3.5 Nothing stands between the model and `bash` — HIGH

**Pi:** `agent/src/agent-loop.ts:600-668` `prepareToolCall` runs `config.beforeToolCall`, which
can rewrite arguments, block with a reason that becomes the model's error result, or terminate;
`examples/extensions/permission-gate.ts:13-32` blocks `rm -rf`/`sudo`/`chmod 777` and calls
`ctx.ui.select("Allow?")` mid-turn; `protected-paths.ts:25` blocks writes to protected paths.

**axum:** `turn.rs:401-406` — a cancel check, then `registry.call(...)`. `registry.rs:120-121`
dispatches straight to `tool.run`. There is no way for the daemon to ask the UI anything:
`UiCommand` (`axum-proto/src/lib.rs:464-503`) has no approval reply and `HarnessEvent` (`:294-380`)
has no request-for-approval variant. The only control that exists is `axum.trusted`
(`config.rs:113-152`), a load-time decision about whether a project's `.axum.lua` may declare
tools at all.

**What it costs:** no confirmation for `rm -rf`, no protected paths, no narrower policy for an
untrusted repo. The controls are all-or-nothing. `config/tools/bash.lua:4` calls shell execution
"the most dangerous thing the model can ask for" and then runs it unconditionally.

### 3.6 Tool calls run one at a time — HIGH

**Pi:** `agent/src/agent.ts:237` defaults `toolExecution` to `"parallel"`; `agent-loop.ts:489-554`
runs them under `Promise.all` and re-orders results to call order; `harness/tools/file-mutation-queue.ts:29-56`
serialises mutations per canonical path.

**axum:** `turn.rs:398-418` is `for call in &calls { … }`, each awaiting the session lock before
the next starts. It is closed, not merely unwritten: `registry.rs:29` `fn run(&self, …) -> Output`
is synchronous, and `registry.rs:11-13` documents the trait as *"deliberately not `Send + Sync`"*
because a Lua tool's VM is neither. No `join_all`/`FuturesUnordered`/`spawn_blocking` anywhere
in the workspace.

**What it costs:** six independent reads cost the sum of their latencies rather than the max,
and with the process transport each is a CBOR round trip (`process.rs:263-330`). The single
largest wall-clock difference per turn on tool-heavy work.

**Shape of the fix that respects the architecture:** the constraint is real and it is per
transport. Process peers are already separate processes and can run concurrently without
touching the `Send` bound; Lua tools stay sequential because their VM says so. Splitting the
dispatch by transport gets the win without contradicting `registry.rs:11-13`.

### 3.7 Tool output does not reach the UI until the tool finishes — HIGH

*Found by the agent-loop and TUI surveys.*

**Pi:** `agent-loop.ts:670-711` passes an `onUpdate` callback into `tool.execute` and emits
`tool_execution_update`; `bash.ts:370-382` snapshots the live tail with its truncation state;
`interactive-mode.ts:3348` calls `component.updateResult(…, true)`.

**axum:** the peer-protocol message exists — `ToolReport::Progress { id, chunk }`
(`axum-proto/src/lib.rs:605`) — and `process.rs:297` consumes it into a local `String` that is
only concatenated on `Result` (`:302-307`). Nothing sends one: `axum-cli/src/shell.rs` never
emits `Progress`; only `examples/peers/echo.c:25` documents it. `registry.rs:29` gives builtin
and Lua tools no progress channel, and `HarnessEvent` has no progress variant for the UI to
receive. `axum-tui/src/transcript/tool.rs:64` renders a body only once a result exists.

**What it costs:** a three-minute `cargo test` shows a spinner and a tool name. The user cannot
tell a slow tool from a hung one, which is precisely the judgement the interrupt key depends on.
The shell peer already has the output line by line at `shell.rs:234-247`.

### 3.8 Compaction fires on characters, not on the tokens the provider reported — MEDIUM

*Found by the agent-loop and prompt-context surveys.*

**Pi:** `compaction.ts:216-241` anchors on the last assistant message's real `Usage` and
estimates only the messages added since; `:247-250` compares against `contextWindow - reserveTokens`;
`:126-130` exposes `reserveTokens` and `keepRecentTokens` as settings.

**axum:** `compact.rs:35` `CHARS_PER_TOKEN = 4` over the whole rebuilt context, compared at
`:62-69` against `HIGH_WATER_PERCENT = 75`. The real numbers are journalled — `turn.rs:265`
stores `usage: turn.usage()` on every assistant entry — but `context.rs:82` sets `usage: None`
when rebuilding, and `compact.rs` never reads one; its only consumer is `session.rs:84-93`,
which sums it for the footer. `KEEP = 8` (`compact.rs:27`) is a fixed *entry* count, so the
retained tail is eight entries whether that is 200 bytes or 8 MB. All three are `const` with no
Lua knob.

**What it costs:** chars/4 is far off for code and JSON, so the proactive path under-fires and
the turn falls to the one-shot reactive `Overflow` retry — which gives up if the kept tail alone
does not fit. In the other direction, eight entries of large tool results can exceed the window
immediately after a compaction, so the session compacts again and loses history it did not need
to. axum already has the accurate number on disk.

### 3.9 The summarisation request is built from the messages that caused the overflow — MEDIUM

**Pi:** `core/compaction/utils.ts:88-89` `TOOL_RESULT_MAX_CHARS = 2000`, applied to every tool
result at `:141-146`, and the conversation is serialised to plain text so the model does not
treat it as one to continue.

**axum:** `compact.rs:88-105` `request()` is `context.messages.iter().take(through).cloned()`
plus one appended instruction — a verbatim clone including every `ToolResult`. `turn.rs:76-92`
sends it straight to the provider.

**What it costs:** compaction fires *because* the context is near the window, and the rescue
call is then the likeliest one to be refused for length. `turn.rs:52-53` treats that failure as
non-fatal and proceeds with the un-compacted context, which then also fails.

### 3.10 No manual compaction — MEDIUM

*Found by the agent-loop and prompt-context surveys.*

**Pi:** `core/slash-commands.ts:39` `/compact`, with optional custom instructions
(`rpc-types.ts:47`).

**axum:** the only callers of `compact()` are `turn.rs:311-313` (proactive) and `:333-337`
(reactive). `UiCommand` has no `Compact` variant and `lib.rs:154-210` has no branch to carry
one. The summariser prompt is the fixed constant `compact.rs:112-123`. The command palette
(`complete.rs:80-96`) has no `/compact`.

**What it costs:** a user who knows a long detour is over cannot free the window. The only lever
is `/rewind`, which discards the exchange instead of summarising it.

---

## 4. D2 — the wire

### 4.1 No prompt caching, anywhere — CRITICAL

**Pi:** `ai/src/api/anthropic-messages.ts:1290-1296` builds `{type:"ephemeral", ttl}` and applies
it to the system blocks (`:1015`, `:1022`, `:1031`), the last tool definition (`:1360`) and the
last user block (`:1305`, `:1312`); OpenAI gets `prompt_cache_key` at `openai-responses.ts:294-295`.

**axum:** axum only reads the counters back. `config/apis/anthropic-messages.lua:119-120` parses
`cache_read_input_tokens`; `axum-model/src/usage.rs:16-18,49-50` prices them; `Usage::cache_hit_rate`
exists. Nothing writes a breakpoint — `grep -rn 'cache_control|ephemeral|prompt_cache' crates/ config/`
returns only read-side hits. `anthropic-messages.lua:74-99` emits model/stream/max_tokens/
messages/system/tools/thinking and nothing else.

**What it costs:** Anthropic never caches without an explicit marker, so on Anthropic — and the
seven other providers routed through `anthropic-messages` — `cache_read` is structurally always
zero. A long session re-pays full input price and full prefill latency on every turn. axum
measures a hit rate it never asks for.

**Shape of the fix:** this one is Lua. `M.request` in `config/apis/anthropic-messages.lua` adds
the markers; `openai-*.lua` adds `prompt_cache_key`. No Rust, no protocol change, and the
cheapest large win on this list.

### 4.2 Nothing streams — the whole message arrives at once — CRITICAL

**Pi:** `ai/src/api/anthropic-messages.ts:427-487` is an async generator that yields each event
as it is parsed off the wire.

**axum:** `axum-provider/src/client.rs:148-155` runs each attempt with `|delta| collected.push(delta)`
and only replays `for delta in collected { on_delta(delta) }` after `Ok(())`. The host does the
same: `turn.rs:157-159` passes `|delta| deltas.push(delta)` and `:183-185` applies them after the
select loop, followed by a single `held.amend(...)` at `:237`. Since `AssistantDelta` is derived
from the grown-text diff of an amend (`session.rs:238-248`), exactly one delta event carrying the
entire message is published per turn — while `client.rs:121-124` and `turn.rs:111-115` both
document the opposite.

**What it costs:** a user watching a 60-second response sees a spinner, then everything at once.
The protocol (`axum-proto/src/lib.rs:332`) and the renderer (`app/mod.rs:303`) were both built
for token-by-token display in M0 and M2. Only the client collects.

### 4.3 The provider's own answer to "when should I come back" is discarded — HIGH

*Found by the agent-loop, providers and Tau surveys.*

**Pi:** `ai/src/utils/provider-retry.ts:51-67` reads `retry-after-ms`, then `retry-after` as
seconds or an HTTP date, validated against a maximum; `utils/retry.ts:7-24` treats
`insufficient_quota` / `quota exceeded` / billing / "Monthly usage limit reached" as permanent
*before* checking the retryable pattern. **Tau:** `tau-provider/src/retry_policy.rs:16` splits
`Throttle` from `UsageWindow` and `Account`, `:94` parses `Retry-After`, `:111` parses
`resets_in_seconds`/`resets_at`, and `tau-ext-provider-builtin/src/lib.rs:2970` holds
`shared_cooldowns` per provider profile.

**axum:** `retry.rs:36-45` maps 429 to `Throttle` unconditionally and `:67-71` makes it
retryable with no exception. `client.rs:231-241` reads only `response.status()` and the body —
the only `headers` call in the crate is `client.rs:220`, setting *request* headers. Backoff is a
blind Fibonacci from `BASE = 10s` (`retry.rs:91`) to `CEILING = 600s` (`:94`), seeded per call,
with `MAX_ATTEMPTS = 4` and no state between calls. `grep -rn 'retry.after|cooldown|quota|billing' crates/ config/`
returns two doc comments and no implementation.

**What it costs:** `Retry-After: 3` waits 10s; `Retry-After: 300` burns all four attempts in
~50s and hard-fails. A 429 that means "your monthly quota is gone" plays the full ladder and a
"Retrying 4/4" status for something no waiting fixes. And with no shared cooldown the next
prompt immediately hammers the same throttled account.

**Note:** this is not Tau's quota pacing chips with hysteresis, which `PLAN.md` §9 refuses.
Reading a header the provider already sent is the opposite of a pacing model.

### 4.4 No sampling parameters — HIGH

**Pi:** `ai/src/api/simple-options.ts:26-33` merges `model.samplingParams` with per-call options;
`types.ts:185-194` documents `temperature` plus a passthrough for `top_p`/`top_k`/`min_p` on
llama.cpp, vLLM and SGLang backends.

**axum:** `axum-provider/src/api/mod.rs:16-25` — `Options { thinking, max_tokens }`, nothing else.
`grep -rn 'temperature|top_p|top_k|min_p|stop_sequences' crates/ config/` returns zero hits
repo-wide. No adapter emits one (`anthropic-messages.lua:74-99`, `openai-responses.lua:47-72`,
`openai-completions.lua:67-`, `google.lua:227-256`).

**What it costs:** no temperature 0 for reproducible runs, no `top_p`/`top_k` for a local
llama.cpp or vLLM endpoint that needs them, no stop sequences. And there is no config key to add
one from Lua either — the neutral `Options` struct is Rust, so this is the one adapter-shaped
gap that is not adapter-shaped.

### 4.5 No structured output or strict tool schemas — HIGH

**Pi:** `ai/src/api/constrained-sampling.ts` in full, consumed by `anthropic-messages.ts:1347-1359`
(`strict: true` plus the schema as `input_schema`), `openai-responses-shared.ts:42`,
`google-shared.ts:19`, `mistral-conversations.ts:382-392`.

**axum:** all four adapters pass `t.parameters` through verbatim with no strict flag and emit no
`response_format` (`anthropic-messages.lua:87`, `openai-responses.lua:59-62`,
`openai-completions.lua:98-101`, `google.lua:241-243`). `grep -rn 'response_format|json_schema|responseSchema|grammar'`
finds three unrelated hits.

**What it costs:** anything needing a machine-readable answer prompts and hopes. Tool arguments
are unconstrained too, which is what makes §4.6 load-bearing. `PLAN.md` §7 chose **schemars**
explicitly to "feed constrained sampling later"; the schemas are there and nothing marks them
strict.

### 4.6 Malformed tool arguments become `null`, silently — HIGH

**Pi:** `ai/src/utils/json-parse.ts:31-60` repairs raw control characters and invalid backslash
escapes inside strings; `utils/validation.ts:302-330` compiles the tool schema, coerces
primitives, and reports localized validation errors.

**axum:** `axum-core/src/turn.rs:44-52` `PendingCall::parsed()` is a bare `serde_json::from_str`.
Both callers discard the error: `axum-core/src/turn.rs:188` and `axum-host/src/turn.rs:404` are
each `call.parsed().unwrap_or(serde_json::Value::Null)`, and `turn.rs:405` hands that `Null`
straight to `registry.call`. No validator dependency exists in any `Cargo.toml`.

**What it costs:** a model that emits a literal newline inside a string argument produces `null`
arguments delivered to the tool as if it had asked for nothing — no error, no retry, and no
message telling the model what was wrong. Pi repairs the common cases and returns a schema error
the model can act on.

### 4.7 No `anthropic-beta` header, and no way to add one — HIGH

**Pi:** `ai/src/api/anthropic-messages.ts:175-176` names the fine-grained-tool-streaming and
interleaved-thinking betas, `:889-899` assembles them per model, `:913`/`:955` emit the header,
`:936` adds `claude-code-20250219` + `oauth-2025-04-20` for subscription tokens.

**axum:** `config/apis/anthropic-messages.lua:23-29` — `M.headers` returns `anthropic-version`
plus `x-api-key` and nothing else. `grep -rn 'anthropic-beta' crates/ config/` — zero. There is
also no per-provider escape hatch: the Adapter trait's only header source is
`fn headers(&self, key: Option<&str>)` (`api/mod.rs:78`), and `config/providers.lua` declares no
`headers` field on any provider.

**What it costs:** on a reasoning model, thinking is dropped between tool rounds rather than
interleaved, so the model re-derives context every round and burns extra thinking tokens on long
agentic loops.

**Shape of the fix:** `M.headers` is Lua. This is a two-line adapter change.

### 4.8 `axum auth login` cannot sign in to anything — HIGH

**Pi:** `ai/src/auth/oauth/` holds eight working flows — `anthropic.ts:29-37` (Claude Pro/Max
client id, authorize/token URLs, scopes, loopback), `github-copilot.ts:154-158` (device code),
plus `openai-codex`, `kimi-coding`, `openrouter`, `radius`, `xai`.

**axum:** the generic machinery is real and complete — `oauth/flow.rs` (PKCE, authorize URL),
`oauth/mod.rs` (0600 store, refresh before use), `auth.rs:17-80` (loopback bind, state check,
browser open, code exchange). But `grep -n 'authorize_url|token_url|client_id' config/providers.lua`
returns nothing: the single `oauth` declaration is `providers.lua:140`,
`auth = { kind = 'oauth', service = 'ChatGPT' }` with no endpoints, so `auth.rs:33-42` bails with
"this build has no sign-in details". Anthropic is api-key only (`providers.lua:25`). No device
code flow exists at all.

**What it costs:** a Claude Pro/Max or ChatGPT Plus subscriber cannot use axum with their
subscription. `PLAN.md` §5d already states this precisely — *"the machinery is done; the
per-vendor details are a line of config away, and untested"* — and it is still true.

### 4.9 Overflow detection is nine substrings, with no exclusions — MEDIUM

**Pi:** `ai/src/utils/overflow.ts:36-63` carries 25 provider-specific patterns, each annotated
with the real error text, plus an exclusion list (Bedrock's `ThrottlingException` says "Too many
tokens"), and `:29-32` documents providers that overflow *silently* — z.ai, detected by
`usage.input > contextWindow`; Xiaomi MiMo, which truncates then returns `finish_reason: length`
with zero output.

**axum:** `retry.rs:73-88` `mentions_length` is nine lowercase substrings promoting
`Invalid|Unknown` to `Overflow` (`:59-63`). No exclusion list — and `"too many tokens"` is in it,
which is exactly Bedrock's throttling wording. No silent-overflow path: `compact.rs:60-69`
compares only a chars/4 estimate against the window and never reads reported usage.

**What it costs:** a provider whose wording is not in the list is classified `Invalid`, so
compact-and-retry never fires and the session dies. A throttle whose body says "too many tokens"
is misread as an overflow.

**Note before anyone widens the list:** `PLAN.md` §6 rule 3 forbids `regex` and `.contains(`
under `axum-provider/src/retry*` precisely because Pi has ~35 retry regexes and re-prefixes
Bedrock errors so they match. More phrases is the wrong direction. The exclusion is free; the
silent-overflow check is `usage.input > context_window`, which is a comparison, not a pattern.

### 4.10 `max_tokens` ignores how full the window already is — MEDIUM

**Pi:** `ai/src/api/simple-options.ts:14-19` `clampMaxTokensToContext` subtracts the estimated
context and a 4096-token margin from the window.

**axum:** every adapter computes `math.min(opts.max_tokens or model.max_tokens, model.max_tokens)` —
`anthropic-messages.lua:78`, `openai-completions.lua:87`, `openai-responses.lua:52`,
`google.lua:231` — with no reference to context fill.

**What it costs:** a 190k-token conversation against a 200k model still asks for 64000 output
tokens, which providers reject outright.

### 4.11 Three declared providers cannot authenticate — MEDIUM

`PLAN.md` §9 says "ship 4" and §Next action already names two of these as unpaid debt. They are
listed here because each currently appears in the model picker and fails when selected.

- **Bedrock.** Pi: `ai/src/api/bedrock-converse-stream.ts`. axum: `providers.lua:167-174`
  declares it, and `grep -rn 'axum.api(' config/` returns 8 registrations, none of them
  `bedrock-converse-stream`. `provider.rs:69` returns "not yet: signing requests with AWS
  credentials". **Not a Lua file** — Bedrock frames with binary AWS eventstream and
  `client.rs:245-262` is SSE-only, so this is transport work.
- **Vertex ADC.** Pi: `providers/google-vertex.ts:6` reads `application_default_credentials.json`.
  axum: the adapter is registered and correct (`google.lua:176`); `provider.rs:70` returns "not
  yet: reading Google application default credentials" and `:96-99` resolves it to `None`.
- **GitHub Copilot.** Pi: `github-copilot-headers.ts:22-36` sends `X-Initiator`, `Openai-Intent`,
  `Copilot-Vision-Request`; `anthropic-messages.ts:901-921` sends Bearer rather than `x-api-key`.
  axum: `providers.lua:43-51` routes it to `anthropic-messages` with `auth = { kind = 'api-key' }`,
  so it sends `x-api-key: <github token>` and no integration headers. No token exchange exists.

**What it costs:** three entries in the model list that are decorative. `PLAN.md`'s own
prescription applies: *"they should be honest about that or be removed."*

### 4.12 The model catalog is a static file — LOW

**Pi:** `ai/src/models.ts:750` `fetchModels`, `:801-818` `refreshModels`, `models-store.ts:3-25`
caches per provider with `etag`/`lastModified`/`checkedAt`.

**axum:** `config/providers.lua` is 42 hand-written entries with fixed ids, windows and costs;
its own header at `:19` says "Model lists are representative, not exhaustive." No HTTP GET exists
anywhere in `axum-provider` — the only request sites are the streaming POST (`client.rs:218-229`)
and the OAuth token exchange.

**What it costs:** each new OpenRouter model needs a hand edit or a release, and the baked costs
go stale silently, so `Usage::price` drifts from reality.

---

## 5. D3 — the session

### 5.1 You can reach exactly one session — CRITICAL

*Found by the TUI and sessions-config surveys.*

**Pi:** `core/session-manager.ts:811` `listSessionsFromDir` and `:174` `SessionInfo` (cwd, name,
messageCount, firstMessage); `modes/interactive/components/session-selector.ts:281` is a
searchable list with rename, delete and sort; `cli/session-picker.ts:14` runs it for `--resume`;
`/resume`, `/new`, `/fork`, `/clone`, `/tree` at `core/slash-commands.ts:22-41`.

**axum:** `axum-host/src/paths.rs:29` `latest()` and `:47` `latest_for(dir, cwd)` are the entire
discovery surface, and each returns one `PathBuf`. `main.rs:44` `--resume` is a bare `bool`.
`UiCommand` (`axum-proto/src/lib.rs:464-503`) has no list request, so the UI could not ask.
`driver.rs:444-496` is the whole command dispatch. The only `Picker` instances are model
(`app/mod.rs:449`) and thinking (`:479`).

**What it costs:** yesterday's session in the same repo is unreachable from inside axum even
though its journal is sitting in `$XDG_DATA_HOME/axum/sessions/`. `PLAN.md` §10 Q1 is still open,
and this is the milestone that settles it.

### 5.2 `/rewind` is a truncation, not a tree — HIGH

**Pi:** `core/session-manager.ts:1360` `branch(branchFromId)` moves the leaf pointer to any
entry; `:1310` `getTree()`; `:1413` `createBranchedSession(leafId)` splits one path into its own
file; `:1232` labels. `tree-selector.ts` navigates it.

**axum:** `axum-proto/src/lib.rs:214-227` `Entry::Branch { keeps: usize }` — "how many entries
from the start remain live" — applied at `context.rs:139-156` as `live.retain(|&i| i < *keeps)`.
`retain` only ever shrinks the current live vector, so a later `Branch` with a larger `keeps`
cannot resurrect an abandoned tail. `axum-journal/src/record.rs:29-35` `Record::Entry` carries a
cursor and no parent pointer. Producers are `/clear` (`driver.rs:446`) and `/rewind` (`:486`).

**What it costs:** a user who rewinds past a good exchange to try something else cannot get the
original back into context; the entries are in the JSONL and nothing can select them. Exploring
two approaches from one point is impossible. This is consistent with `PLAN.md` §5d — the record
was always described as linear — but "append-only" does not require "linear", and the DAG is the
retrofit that gets harder with every journalled session.

### 5.3 There is no way to find a past session by what was said in it — HIGH

**Pi:** `session-backends/sqlite-node/src/sqlite/search-backend.ts:66-88` builds an FTS5 trigram
index over entry payloads; `session-selector-search.ts:26` searches id + name + all message text
+ cwd, with fuzzy, phrase and regex modes.

**axum:** nothing reads a session body for matching. `paths.rs` reads only the first (meta) line
of each journal. `axum-tui/src/fuzzy.rs` serves the completion popup and the pickers only. The
two mentions of search in the tree are aspirational: `scrollback.rs:5` and `terminal.rs:32`.

**What it costs:** with no listing and no search, finding "the session where I fixed the CBOR
framing" means grepping `$XDG_DATA_HOME` by hand, outside the tool.

**Shape of the fix, today:** the journal is greppable JSONL by design (`PLAN.md` §7 — *"`less` is
the debugger"*). A **process peer** that greps the sessions directory closes the model-facing
half of this with no Rust at all. The user-facing half needs §5.1 first.

### 5.4 No scripted mode — one prompt, one blob of text — HIGH

**Pi:** `modes/rpc/rpc-types.ts:20-73` is a ~35-command union — prompt, steer, follow-up, abort,
model, thinking, compact, bash, fork, session switching, tree and entry queries — served by
`rpc-mode.ts` over JSON lines on stdio; `cli/args.ts:11` `Mode = "text" | "json" | "rpc"`.

**axum:** `axum-cli/src/print.rs:39` sends one `Attach` and one `SubmitPrompt` and folds events
into `Outcome { text, stop_reason, error }`; `main.rs:139-156` prints `outcome.text` and exits.
No JSON event emission. The rich control protocol exists but is internal: `UiCommand` as CBOR
over a Unix socket, with no stdio front end and no `serve`/`rpc` subcommand in `main.rs:64-101`.

**What it costs:** an editor plugin or a script cannot observe tool calls, steer, abort, or
switch models without reimplementing the CBOR framing. Everything the mode needs already exists
one layer down; what is missing is a stdio adapter over it — which is a *client*, not a third
transport, so it does not touch M9's settled rule.

### 5.5 Cost is computed on every turn and shown nowhere — MEDIUM

**Tau:** `tau-session-inspect/` produces per-session and per-agent activity stats, per-tool
counts, model and effort breakdowns, and OTLP export.

**axum:** `Usage::price(Cost)` is implemented (`axum-model/src/usage.rs:45`) and
`config/providers.lua:27-29` carries real per-model prices — and the only callers of `price` are
in its own unit test at `usage.rs:135-137`. The subcommand list (`main.rs:64-101`) has no
inspect, stats or export verb, and the footer never renders cost.

**What it costs:** a user cannot ask what a session cost, which tools it called, or where the
tokens went, even though `Usage` is journalled per turn and every price is known. Answering "why
was this month expensive" means writing a `jq` script.

**Shape of the fix:** the data is on disk in greppable JSONL. A **process peer** or a small
`axum` subcommand reads it. No daemon change, no protocol change.

### 5.6 The Lua API ships a client and no server — MEDIUM

**Pi/Tau:** Pi's `packages/server` + `packages/client` let another process attach to a live
session and read its transcript and status.

**axum:** the client half is shipped — `axum-lua/src/stub.rs:15` embeds `lua/axum.lua`, which
declares `SURFACE = sessions, session, verbs, status, transcript, cwd, models` (`:290-296`) and
tells callers to obtain it via `io.popen("axum lua-api")` (`:9`). There is no `lua-api`
subcommand (`main.rs:64-101`), and grepping `crates/` for uses of `stub::CLIENT` outside its own
test returns nothing. `peer.rs:54` scans for `api@*.sock`; the only binds are `socket_for(cwd)`,
`<runtime>/axum/<fnv-hash>.sock` (`axum-ipc/src/lib.rs:78-86`) — never an `api@*.sock`.

**What it costs:** a sibling tool that loads the stub and calls `agent.sessions()` gets nothing.
The advertised API has no listener, so the status-bar integrations it documents cannot be
written. This is §1's pattern in its purest form: a whole client library with no server.

### 5.7 An extension can add a tool and nothing a user touches — MEDIUM

**Pi:** `core/extensions/types.ts:1295-1307` `registerCommand`, `registerShortcut`, `registerFlag`,
`registerTool`; the UI context at `:133-283` adds status widgets, footer, header, autocomplete
providers, editor components and themes.

**axum:** tools are covered twice over — in-daemon Lua (`engine.rs:209-220`) and process peers
(`ext_lua.rs:24-40`, `config/peers/greet.lua`) — and providers and wire protocols via
`engine.rs:194-205` and `:264`. Nothing else: slash commands are `driver.rs:444` plus the fixed
`complete.rs:82` list; keys are the fixed match in `keys.rs`; CLI flags are the clap derive at
`main.rs:30-62`; every TUI surface is compiled in.

**What it costs:** an author can add a tool the *model* calls but nothing the *user* can invoke —
no command, no key, no status line. The two mechanisms M9 proved are complete on the model side
and empty on the user side.

### 5.8 Sessions have no name — MEDIUM

**Pi:** `core/session-manager.ts:1136` `appendSessionInfo(name)` writes a `session_info` entry;
`/name` at `slash-commands.ts:29`; filterable by named-only.

**axum:** `axum-journal/src/record.rs:17-35` — `Record` has exactly `Meta { version, session, cwd, started }`
and `Entry { cursor, entry }`. No name field, and no record type that could carry one.
`paths.rs:24-27` `session_id()` is a zero-padded unix timestamp. `UiCommand` has no rename.

**What it costs:** sessions are identified by a 20-digit timestamp, so even once §5.1 exists
there is nothing to recognise them by.

### 5.9 The first journal format change orphans every session on the machine — MEDIUM

**Pi:** `core/session-manager.ts:281-291` runs `migrateV1ToV2` then `migrateV2ToV3` against
`CURRENT_SESSION_VERSION`.

**axum:** `axum-journal/src/lib.rs:96-101` returns `JournalError::Version` and refuses the open
whenever `meta.version != JOURNAL_VERSION`. `record.rs:7` pins it at 0 with the comment *"Stays
0: while axum is the only reader, breaking it is free."* The crate is lib.rs + record.rs +
recovery.rs, and recovery handles torn tails, not versions.

**What it costs:** the comment is honest and correct *today*. It stops being correct the first
time a user has sessions worth keeping — and the error path is a hard refusal, not an upgrade.
Worth recording now so the decision is made deliberately rather than discovered.

### 5.10 No export, no import — MEDIUM

**Pi:** `core/session-export.ts:7` `exportSessionToJsonl`, `/export` and `/import` at
`slash-commands.ts:24-25`.

**axum:** no `/export` or `/import` in `driver.rs:444-496`, no subcommand in `main.rs:64-101`.
The journal is version-gated private JSONL, not an export target.

**What it costs:** a session cannot be attached to a bug report or moved between machines.
`PLAN.md` §9 refuses **HTML export** and **a second session format**, which is most of Pi's
version of this. What is left and not refused is copying the JSONL out and reading one back —
which the append-only design makes nearly free.

### 5.11 No sharing to a URL — MEDIUM

**Pi:** `modes/interactive/session-share.ts:45` shares through Radius, falling back to
`gh gist create --public=false` at `:168`; `/share` at `slash-commands.ts:27`.

**axum:** no gist/share/publish/upload code in `crates/` or `config/`. The only network clients
are the provider HTTP path and the OAuth loopback bind (`auth.rs:47`).

**What it costs:** showing a colleague what the agent did means screenshots or manual copying.
This is the one gap on this list that a **process peer** closes outright today, and should: a
peer that shells out to `gh` needs no daemon change at all.

---

## 6. D4 — the peers

### 6.1 The shell hands the command its own control pipe — CRITICAL

**Tau:** `tau-ext-shell/src/pty_stdio.rs:44` and `shell_process.rs:59` both attach
`Stdio::null()` to every command, so a command that reads stdin gets EOF.

**axum:** `axum-cli/src/shell.rs:139-145` spawns the long-lived `sh` with `.stdin(Stdio::piped())`,
and `:220` writes the command *and* its end-of-command marker into that same pipe:

```
let script = format!("{{ {command} ; }} 2>&1\nprintf '%s%s\\n' \"{marker}\" \"$?\"\n")
```

Nothing redirects the command's stdin. `grep -rn 'Stdio::null|< /dev/null' crates/` finds only
unrelated hits in `daemon.rs`, `auth.rs` and the testkit; `process.rs:134-145` spawns peers the
same way.

**What it costs:** verified by reproduction. `{ cat ; } 2>&1` followed by the marker `printf`
makes `cat` swallow the marker line and the entire next command. The tool call then never sees
its marker and blocks for the full `CALL_TIMEOUT = 600s` (`process.rs:30`), and the persistent
shell's queued state is destroyed. Any model-issued `cat` with no file, bare `grep pattern`,
`ssh host`, `sudo`, `npm login` or `git rebase -i` wedges the session for ten minutes.
`crates/axum-cli/tests/bash.rs:107,151` only ever run `cat <file>`, so no test covers it.

**Shape of the fix:** one redirect in the generated script, or `Stdio::null()` on the group. It
is a line, and it is the cheapest critical on this list.

### 6.2 A peer can answer a call and observe nothing else — HIGH

**Tau:** `tau-proto/src/messages.rs:75` `Subscribe { historical_selectors, live_selectors }`,
`:1388` `Emit`, `:1438` `Deliver`. `tau-ext-std-notifications/src/lib.rs:643-647` subscribes to
`provider.response_finished` / `agent.prompt_submitted` to fire a desktop notification when a
turn ends.

**axum:** the entire peer vocabulary is `ToolRequest::{Call, Cancel}` (`axum-proto/src/lib.rs:568`)
and `ToolReport::{Declare, Progress, Result}` (`:591`). `Subscribe`/`Attach` exist only on the UI
socket (`:465`) and `process.rs` speaks neither.

**What it costs:** nothing outside the daemon can observe a session. No desktop notification or
terminal bell when a long turn finishes, no transcript shipped to an external log, no automation
on turn-end. Every such feature becomes new Rust in the daemon — which is exactly the growth the
out-of-process peer design exists to prevent.

**Note before anyone builds it:** `axum-proto` is capped at 4,000 lines (`PLAN.md` §1.1), and Tau's
165 event variants are named there as the direct cause of its 34,875-line daemon. The version
worth having is a narrow selector set over events that already exist, not Tau's.

### 6.3 A peer has nowhere to put state — HIGH

**Tau:** `tau-proto/src/messages.rs:943-959` `ExtensionDataScope::{Session, User, Cache, Secret}`,
`:963` `ExtensionDataRequest`, `:1446` `ExtensionDataResult` — the harness reads and writes on the
extension's behalf, so an extension needs no filesystem access and its credentials sit under a
harness-only root.

**axum:** no storage operation on the wire. In-daemon Lua tools are sandboxed with `io`,
`package`, `dofile`, `loadfile` and `require` removed (`axum-lua/src/sandbox.rs:20-30`) and the
only filesystem primitive is `fs.ls` (`axum-lua/src/fs.rs:15-56`) — name and mtime, no read, no
write. Process peers are spawned at `process.rs:134-145` with no `.env()` and no state directory.

**What it costs:** a tool that needs an API token, a cache, or any memory between calls has
nowhere to put it. In-daemon Lua literally cannot open a file; a process peer must reach around
the protocol to the raw filesystem with no scoping and no ownership. That rules out the whole
class of peers that talk to a third-party service.

**Shape of the fix:** the cheap version needs no protocol change at all — hand the peer a state
directory in its environment at spawn (`process.rs:134-145` sets none today). Tau's four scopes
are the expensive version and can wait for a peer that needs secrets.

---

## 7. D5 — what the model is told

### 7.1 No skills — HIGH

**Pi:** `core/skills.ts:407-506` loads `SKILL.md` from a user dir and a project dir, validates
name and description, `:355-381` emits an `<available_skills>` block into the prompt so the model
loads the body only when the task matches, and each is also an explicit `/skill:name` command.

**axum:** case-insensitive grep for `skill` across `crates/`, `config/` and `PLAN.md` returns
zero. `system.rs:33-42` `assemble` pushes exactly instructions, environment and `AGENTS.md` — no
discovery step. `config/system.lua:13-29` is one static string.

**What it costs:** a repeatable procedure — a release checklist, a migration recipe, a house
review process — has nowhere to live except `AGENTS.md`, which is capped at 32k (`system.rs:26`)
and ships on **every** request whether relevant or not. Progressive disclosure is the whole point
of the mechanism and axum has the opposite.

### 7.2 A frequently-used prompt cannot be saved — HIGH

*Found by the prompt-context and sessions-config surveys.*

**Pi:** `core/prompt-templates.ts:194-263` loads `.md` files from a global and a project
`prompts/` dir; `:70-102` `substituteArgs` supports `$1`, `$@`, `$ARGUMENTS`, `${N:-default}`,
`${@:N:L}`; they surface as commands alongside skills and extension commands
(`slash-commands.ts:4`).

**axum:** `driver.rs:446-493` is a closed match over six literal names with `_ => unknown command`;
`complete.rs:80-96` is the matching hardcoded palette. Nothing reads a directory. No expansion
syntax anywhere.

**What it costs:** every repeated task — "review this diff for X", "write a conventional commit
for the staged changes" — is retyped in full each session, and a team cannot check a shared
command into the repo.

**Note:** this needs a command registrar, which is the same decision §3.3 flags. Lua cannot do it
today because `fs.ls` cannot read a file.

### 7.3 A peer cannot ship the instructions that explain it — MEDIUM

**Tau:** `specs/SPEC-prompt-fragment-declarations-and-projection.md:10-15` — every configured
extension may publish prompt fragments, keyed by name and priority, which the harness projects
into the assembled prompt (`tau-proto/src/prompt_fragment.rs:93-97`).

**axum:** `system.rs:33` `assemble(instructions, cwd, now)` takes no tool information; the prompt
is config text plus the environment block plus `AGENTS.md`. `ToolReport::Declare` carries name,
description and parameters only.

**What it costs:** a peer's whole contract with the model is one tool description. A peer that
needs conventions stated up front — a deployment rule, a severity vocabulary, a domain glossary —
requires the user to paste them into `config/system.lua` by hand, so shipping a peer is never
self-contained and two users of the same peer get different behaviour.

**Shape of the fix:** one optional field on `ToolReport::Declare`, which already exists and
already wins over config (`PLAN.md` §5h). Bounded, and it does not grow the event surface.

### 7.4 The prompt names tools whether or not they exist — LOW

**Pi:** `core/tools/read.ts:27-30` and `bash.ts:47-50` — each tool contributes a snippet and
guideline bullets; `core/system-prompt.ts:81-84` builds the Available-tools list from the enabled
set; `agent-session.ts:1045-1078` rebuilds the prompt when the set changes.

**axum:** `system.rs:33` takes no tool parameter. `config/system.lua:16-17` names `edit`, `write`,
`read` and `bash` in unconditional prose. `turn.rs:41-45` documents the prompt as assembled once
at daemon start and `:113` only clones it.

**What it costs:** removing or renaming a tool leaves the prompt telling the model to use it
anyway, and an operator who adds a peer tool can only teach the model about it through the schema
`description`. `PLAN.md`'s Next action already names the sibling problem — the prompt is
assembled once and an edited `AGENTS.md` mid-session is silently stale — and the fix it proposes,
*"a reload the transcript can show"*, is the same mechanism.

---

## 8. D6 — the screen

Ordered by severity; all of these are `axum-cli` and `axum-tui`, none of them touch the daemon.

### 8.1 Code is one colour — HIGH

**Pi:** `modes/interactive/theme/theme.ts:1180` `highlightCode(code, lang)`, called from
`tui/src/components/markdown.ts:523` and from `read.ts:186` / `write.ts:67` for file bodies.

**axum:** `axum-tui/src/markdown.rs:58-61` paints every fenced line with one colour,
`theme.md_code_block`; `:102` renders the language tag as a label on the bar and nothing more.
No highlighting dependency in any `Cargo.toml` — grep for `syntect`, `tree-sitter`, `two-face`,
`synoptic` returns zero.

**What it costs:** every code block the model emits and every file it reads back is a monochrome
wall. Scanning a 40-line snippet for the changed expression is materially slower than in Pi. M8
built the fenced-block frame; this is the content inside it.

### 8.2 Six commands — HIGH

**Pi:** 23 built-ins at `core/slash-commands.ts:20-42`, plus prompt-template, extension and skill
commands merged in.

**axum:** `complete.rs:80-86` — `/help`, `/clear`, `/model`, `/rewind`, `/think`, `/quit`, with
the source comment at `:77` reading "Deliberately short. Pi has 28." `driver.rs:444-500` matches
exactly those. `help.rs:29` builds the help text from the same list by design, so there is no
second hidden set.

**What it costs:** compacting on demand, starting a new session, copying the last reply,
exporting a transcript, reloading config — none of them has a way in. The comment is a deliberate
starting point, not a ceiling, and several sections of this document land here.

### 8.3 The rest — MEDIUM and LOW

| gap | Pi | axum |
|---|---|---|
| **One palette, and `axum.theme` is inert** | `theme-controller.ts:18` with light/dark JSON, terminal-background detection at `:64`, and a `/theme` selector | `theme.rs:61` `DARK` is the only palette, `:87` `Default` returns it, `driver.rs:40` is the only construction. `config.rs` reads `trusted`/`system`/`thinking`/`model` — `theme` is never looked up, and `axum-lua/src/tests.rs:20-26` asserts it is absent |
| **No rebinding** | `tui/src/keybindings.ts:8-58`, 57 named bindings, user-overridable; `/reload` re-reads them | `keys.rs` is a hardcoded match; `help.rs:3-5` says so twice — *"a key binding is a `match` arm and a match cannot describe itself."* `axum.keys` harvests fine (`tests.rs:31`) and nothing reads it |
| **No `!` bash mode** | `interactive-mode.ts:2904` detects a leading `!`, `:4138` recolours the editor border; `bash-execution.ts:21` streams the output | `keys.rs:179,209` test only `starts_with('/')`; everything else is a prompt. `shell.rs` is the model-callable peer, not a prompt mode |
| **No clipboard** | `interactive-mode.ts:6115` copies the last agent message; `/copy` at `slash-commands.ts:28` | no `clipboard`/`arboard`/`copypasta`/OSC 52 anywhere; paste is inbound only (`terminal.rs:87`, `driver.rs:209`) |
| **No transcript search** | `tui/src/alt-screen-search.ts` builds a corpus from rendered lines; `tui-alt-screen.ts:579-592` search/next/previous, `:626` previous prompt | `scrollback.rs:41-119` is set/len/view/scroll/page/top/bottom. Its own comment at `:5` says the owned buffer *"is also what makes transcript search, selection, and jump-to-message possible later"* |
| **No mouse** | `tui-alt-screen.ts:55-57` enables SGR mouse; `:747` scrollbar drag, `:957` selection, `:697` right-click paste | `terminal.rs:86-89` enables bracketed paste and the alternate screen only; `driver.rs:209-216` drops everything that is not Key/Paste/Resize. No `EnableMouseCapture` in the tree |
| **Diffs have no line numbers and no word-level detail** | `components/diff.ts:8` parses `+123 content`; `:26` `renderIntraLineDiff` inverses the changed tokens | `builtin.rs:179-194` emits a bare `-`/`+`/space per line with no numbers and no hunk headers; `transcript/tool.rs:104-115` colours by first byte. `similar` is already a dependency and `iter_inline_changes` is never called |
| **`/model ` completes nothing** | `interactive-mode.ts:685` `getArgumentCompletions` for models, `:711` thinking levels, `:724` login providers; `:2456` extensions add their own | `complete.rs:107-109` closes the popup at the first whitespace; `Kind` is `Command` or `Path` (`:18-22`); `resolve` (`:100`) takes one `list_paths` closure |
| **Signing in means quitting** | `components/login-dialog.ts:11`, `oauth-selector.ts`, `first-time-setup.ts`; `/login` and `/logout` | `main.rs:70-72` — `axum auth login|logout|status` only. No `/login`, no auth branch in `driver.rs`, and `greeting.rs:29-40` offers four keys and no setup path |
| **`/rewind N` is an index you cannot see** | `components/user-message-selector.ts:110` lists the session's user messages to pick from | `driver.rs:487-497` parses a raw `usize`; `axum-proto/src/lib.rs:493-501` `Branch { keeps }` is entry-count based and its own doc concedes *"only the daemon knows which entries are still live"* |
| **No settings menu** (LOW) | `components/settings-selector.ts:438` plus submenus | one unrelated doc-comment hit for "settings" in the whole UI. Every knob is `config/init.lua` plus `axum stop`, which `config.rs:340-353` exists to warn about |
| **Links are raw markdown** (LOW) | `tui/src/components/markdown.ts:689-691` renders OSC 8 hyperlinks | `markdown.rs:4-5` says Pi *"also does tables, links with URL dimming, and LaTeX; those wait until a transcript needs them."* M8 shipped tables. Links did not |

---

## 9. D7 — the tools

### 9.1 `read` cannot page and has no byte cap — HIGH

**Pi:** `core/tools/read.ts:22-25` takes `offset` and `limit`; `:218` tells the model output is
truncated at 2000 lines *or 50KB, whichever is hit first*, and to continue with `offset` until
complete; `truncate.ts:78-160` enforces it.

**axum:** `builtin.rs:12` `PREVIEW_LINES = 2000` is the only limit and `:41-48` the schema is
`{ path }` with nothing else. On overflow, `:69` tells the model to "ask for them with a shell
command if you need them". No byte measurement anywhere on the path.

**What it costs:** a minified bundle, a lockfile, or a generated single-line JSON is returned in
full and blows the window on one call. There is no in-tool way to read past line 2000, so the
model is pushed to `sed -n` through `bash` — which is §3.1, uncapped.

### 9.2 `edit` changes one site per call — MEDIUM

**Pi:** `core/tools/edit.ts:45-54` takes `edits[]`, all matched against the original content and
applied atomically in one write with one diff; `:56-64` tells the model to use one call for
multiple locations.

**axum:** `builtin.rs:128-138` declares scalar `path`/`old`/`new`; `:150-174` counts matches once
and does a single `replacen(old, new, 1)` then one `ops.write`. `turn.rs:398` iterates calls
sequentially, so N edits are N tool calls and N writes.

**What it costs:** renaming a symbol at eight call sites in one file costs eight round trips
through the model. It is also non-atomic — a failure at edit five leaves the file half-changed
with no rollback.

### 9.3 `edit` cannot match a CRLF file at all — MEDIUM

**Pi:** `core/tools/edit.ts:363-370` strips a BOM before matching — *"the model will not include
an invisible BOM in oldText"* — normalizes to LF, and restores the original line ending and BOM
before writing.

**axum:** `ops.rs:118` `read` is `std::fs::read_to_string`, which preserves both a UTF-8 BOM and
CRLF; `builtin.rs:152` matches against exactly those raw bytes; `ops.rs:126-131` writes back what
it is handed. No `\r`, `BOM` or `feff` anywhere in `axum-tools` or `axum-lua`.

**What it costs:** a loop the tool cannot escape from inside. `read` shows the model LF-only text
(`builtin.rs:59` uses `str::lines()`, which strips the `\r`), so the model sends back multi-line
`old` joined with `\n`, which never matches the `\r\n` on disk. The error tells the model to
re-read the file, which shows it the same LF text again. A leading BOM breaks any edit touching
line 1 the same way.

---

## 10. What axum deliberately does not have

These are gaps. They are also decisions, already written down. **Do not build them because they
appear in this document.**

- **Images, end to end.** Found by four of the seven surveys. `Content::Image` exists
  (`axum-model/src/lib.rs:81`), three Lua adapters encode it, `accepts_images()` reports which
  models take one — and nothing constructs one, no paste path accepts one, and `Output` is a
  `String` so no tool could return one. `PLAN.md` §4 M10+ and its Next action both state this as
  described-not-claimed. **One thing here is worth fixing regardless:** the failure would be
  silent, not a refusal. `openai-responses.lua:14-43` has no image branch, and
  `openai-completions.lua:31-32` pushes an empty string with the comment "images ride as parts
  below" where no parts block follows. If an image ever enters, three providers drop it quietly.
- **Multi-agent / subagent delegation.** Pi ships it as an *example extension*
  (`examples/extensions/subagent/index.ts`), not core. `PLAN.md` §4 M10+ parks it. axum's peer
  layer is where it would belong if anything ever demands it, and nothing has.
- **HTML export and a LaTeX renderer.** `PLAN.md` §9, explicit. The JSONL half of export is a
  separate question — see §5.10.
- **Quota pacing chips with hysteresis.** `PLAN.md` §9, from Tau, explicit. §4.3 is not this:
  reading a `Retry-After` header the provider already sent is the opposite of modelling a quota.
- **Searching upward for `AGENTS.md`, and accepting five filenames.** Pi walks from cwd to root
  and accepts `AGENTS.override.md`/`AGENTS.md`/`AGENTS.MD`/`CLAUDE.md`/`CLAUDE.MD`
  (`core/resource-loader.ts:71-72,119-156`). `PLAN.md` §5f refuses both on purpose — *"one
  filename, not a search across five, read from the session root only — a file two directories
  above the one you are working in is a file you did not know you were agreeing to."* **The
  defect is the silence, not the policy:** starting axum in `packages/api/` of a monorepo drops
  the repo-root file and says nothing (`system.rs:86` is a single `cwd.join`). A notice naming
  what was found, or not found, keeps the decision and removes the surprise.
- **A `kind = "command"` transport.** `PLAN.md` §5h, settled, with the argument on both sides
  already written down. Nothing in this document needs one — §5.3, §5.5 and §5.11 all land on the
  process peer that already exists.

---

## 11. Next

Ranked by what a user hits soonest, not by what is most interesting to build. The first two are
each roughly a line of code and each currently ends a session.

1. **Isolate the shell's stdin (§6.1).** A bare `cat`, `sudo`, or `ssh` wedges the tool for the
   full 600-second timeout and destroys the persistent shell's state. One redirect.

2. **Bound tool output at the chokepoint (§3.1).** One `cat` of a lockfile is unrecoverable,
   because the result is journalled, replayed on every request, inside the `KEEP = 8` tail that
   compaction cannot touch, and fed to the summariser. This is the only gap on the list that
   destroys a session permanently.

3. **Move the compaction cut off tool boundaries (§3.2).** Every long tool-heavy session ends in
   a 400 that `retry.rs:40` classifies `Invalid` — not retryable, not `Overflow` — so nothing
   recovers. `covers()` needs to see the entries it is cutting.

4. **Publish deltas as they arrive (§4.2).** The protocol, the renderer and the doc comments were
   all written for streaming in M0 and M2. `client.rs:148-155` and `turn.rs:157-159` collect into
   a `Vec` first. Every turn currently looks hung, and this is §1's pattern in the most visible
   place in the product.

5. **Emit cache breakpoints (§4.1).** Pure Lua, in the adapter axum ships first, on the provider
   axum ships first. Nothing else on this list changes cost and latency by that factor for that
   little work.

Everything else waits for something concrete to hit it — which is the rule `PLAN.md` §5e states
and which this document exists to serve, not to replace.
