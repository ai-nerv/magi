<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="misc/magi_alt-dark.svg">
    <source media="(prefers-color-scheme: light)" srcset="misc/magi_alt.svg">
    <img src="misc/magi_alt.svg" alt="magi" width="520">
  </picture>
</p>

<p align="center"><em>A coding agent for Linux.</em></p>

<p align="center">
  <a href="https://claude.ai/code/artifact/cf3ff7f0-1c1d-472a-b01e-d08a854178b1"><strong>How the four fit together</strong></a> —
  a turn end to end, writing a tool, memory both directions, what may run
</p>

Three computers had to agree before NERV moved. This is the one you talk to: the harness — a
constellation of POSIX processes over Unix sockets, and a terminal that earns the name, with
differential rendering into native scrollback, live streaming and an editor-grade prompt. Close
the window and the turn keeps running.

## The family

The MAGI of *Neon Genesis Evangelion* were three units deliberating, each running a facet of the
same mind, and no one of them the whole. Here they are three programs, in three repositories,
that talk over sockets and pipes:

| | |
|---|---|
| **magi** | this one: the harness — UI, host and turns; models come through melchior, tools from casper |
| **melchior** | the agent layer — sessions talking to sessions, adoption, permissions |
| **balthasar** | the memory layer — what was said, distilled and recalled |
| **casper** | the tools, and the screen they draw on |

**They are separate programs, not components.** melchior does not know what a harness is, and
balthasar's Rust never parses magi's types — each one is useful, and testable, with the others
absent. magi with no melchior is a session with no siblings, which is the ordinary case and not
an error.

## Status

A real agent: a model answers, tools run in their own processes, and the session is held by
balthasar — which is also what makes it resumable, and what decides when it is compacted. magi
keeps no transcript of its own. See `PLAN.md` for how it was built.

```sh
make run          # magi, for real, in the current directory
make install      # the static binary into $PREFIX/bin, so `magi` works anywhere
make configs      # install config/ into ~/.config/magi, ready to edit
make build        # the release binary, static where the toolchain allows it
make verify       # fmt, check, test, clippy, gates, docs
```

**Both are needed.** `make configs` installs configuration; it does nothing for a binary you
have not installed. `make configs` says so when the two are out of step.

Configuration synchronization is file-by-file: the newer differing side wins. A newer installed
file is copied back into the checkout and named in the output. Equal timestamps favor the
checkout; identical files use its timestamp. `make configs --keep` leaves differing installed
files alone. `--dest=PATH` selects another destination, and extra installed files are retained.
`make test-config-install` exercises the actual recipe in disposable source/destination trees.

With no API keys set, `make run` says as much and `/model` lists every model magi knows with
what each would need — so the first thing you do is choose one rather than read a config file.

Two backends, one renderer. `alt` takes the alternate screen and owns the transcript, which is
what transcript search and selection will need; `inline` keeps a live region at the bottom of
the normal screen and lets the terminal keep the history, so native scroll, search, and copy
keep working. Both draw from the same components and answer the same keys, and the footer names
whichever is active.

`make run` starts a daemon for the working directory and attaches the UI to it over a Unix
socket — two processes, as the architecture intends. Quitting the UI detaches; the turn keeps
running, and `magi stop` ends the daemon.

For working on the interface without a model, `make demo` replays a recorded session:

```sh
make demo         # the UI against a canned recording — no model, no tools
make host         # a replay host alone
make ui           # the UI alone, attaching to it
```

## Workflow skills

Use `/skill <name> [literal arguments]` to select a Markdown workflow. Project
files live in `.magi/skills/<name>/SKILL.md` or `.agents/skills/<name>/SKILL.md`;
user workflows live under the XDG Magi configuration directory. `:skills` refreshes
completion. Selected content is frozen in history and grants no tool permissions.
See [Workflow skills](WORKFLOWS.md) for precedence, format, bounds, print-mode use,
and current verification limits.

## Tools

Four kinds, and the registry does not care which is which: a transport is a property of a
declaration rather than a second registry, so each is checked against the same schema, asks the
same person for the same permission, and is capped and masked on the way back.

| Kind | What it is |
|---|---|
| `supplied` | from the tools program (casper), over its command or persistent socket door. Machine tools and external MCP tools |
| `builtin` | compiled in: `spawn` — magi's only one, and coordination, not machine work |
| `lua` | a function in the config's own VM — how the memory tools reach balthasar |
| `command` | one exec per call, arguments built from the call — how `agent` reaches melchior |

The machine tools all arrive from **casper**, which is another program and supplies the whole set.
That makes it the largest trust assumption magi makes, and `magi.casper_sha256` pins it to the bytes
you set it up against. magi runs no tool of its own beyond `spawn`; there is no in-magi shell, no
process peer or direct MCP transport — running a tool is casper's. Casper discovers server
definitions from `$XDG_DATA_HOME/casper/mcp/*.json` (default `~/.local/share/casper/mcp/`).
No other application's registry or project folder is scanned.
`tools` with `path = "mcp"` lists configured servers and discovered tools. MCP calls
prefer its persistent socket door to retain server sessions. A lost remote response is not
replayed through the command door. `MAGI_TOOLS_DOOR=command` explicitly chooses one-shot sessions.

See Casper's [MCP client setup](https://github.com/ai-nerv/casper/blob/main/docs/MCP_CLIENT.md)
for stdio, HTTP and Blender examples.

casper is the default for the `tools` role, not a requirement: `magi.tools = "workbench"` hands the
role to any program that answers `ROLES.md`'s core, and `magi.workbench_sha256` pins that one
instead. Only your own configuration can name it — a project file that tries is refused.

```sh
magi tools        # what the model can call, and how each is reached
magi doctor       # what a session here would be made of, without starting one
```

`magi doctor` answers everything a session decides at start-up: which configuration was read,
which of its lines were kept, what the registry ends up holding and where each entry came from,
and whether the siblings are actually *answering* — asked, not looked for, because a program on
`$PATH` is not a running one and a socket that accepts is not one that answers. It works on a
machine where nothing is installed, which is the machine you run it on.

## Memory

With balthasar running, a turn is shown what the project already remembers before it starts —
without the model having to think to ask, which a model that has forgotten something cannot do.
What is *current* is stated; what is merely on record is hedged, because balthasar decides which
a memory is and flattening the two would present something you said once in March as true now.

It is bounded and on a clock: a tenth of the window, and 250ms to answer. A memory layer that is
slow, wedged or absent costs the conversation nothing, which is what lets this be unconditional
rather than a setting you have to find.

What the turn then *does* goes back — the only signal balthasar has for whether anything it
offered was worth offering, and the axis MemoryArena separates from LoCoMo.

## How this family talks

Three transports, two shapes, one encoding — written out because it was written out nowhere, and
five wires had grown five ways to say the same thing.

| Transport | When | Framing |
|---|---|---|
| **argv** | a question with an answer and nothing to hold open | one JSON object on stdout |
| **pipe** | a parent and the child it started | newline-delimited JSON, both directions |
| **socket** | a sibling of this user's, checked by `SO_PEERCRED` | four bytes of big-endian length, then at most 16 MiB of JSON or CBOR |

JSON is on all three; a socket body may also be CBOR, the same shape as bytes. magi's own session
socket is the exception: its own CBOR protocol for a screen, where a connection that does not
present the key in `<socket>.key` watches and changes nothing.

A **call** is answered; an **event** is not:

```
->  {"call":"status","args":[]}
<-  {"ok":true,"family":1,"n":1,"result":[{"busy":false}]}

    {"event":"listening","at":"…"}
```

`result` is a **list** and `n` says how long it is: a sibling that unpacked a bare value would
read an answer as nothing at all. `family` says which revision the reply is written in — a reader
refuses a number it does not know and tolerates one it predates. A refused call is a *reply*, not
a dropped connection.

**The tag key is `event`, everywhere, in both directions**, and `gate-wire` refuses any other.
The failure it prevents is silent: two of these wires exist as byte-identical copies in two
repositories, so when two spellings drift nothing fails and no test goes red — the surface simply
stops being answered.

## Talking to other sessions

### Concurrent clients

All attached clients share one session queue; only a client that attached with the key in
`<socket>.key` may add to it, and one without it watches. Prompts, incoming messages, permission
declarations and inherited grants are admitted in order. Prompts received during a turn
wait for its completion and persistence attempt; they do not interrupt it. Consecutive
incoming-agent messages at a boundary are read together. Up to 256 requests may wait;
additional requests receive an explicit refusal instead of being silently dropped.

Interrupt requests cancellation of the active operation only. Queued prompts retain their order and receive
fresh cancellation state. Model, provider, thinking, branch and resume changes are refused
while work is active or queued. Disconnecting a client does not cancel work already accepted
by the running host. The queue is in memory, not crash-durable; terminating the host loses
requests that have not reached the transcript. A stopped worker produces a transcript error
for each accepted prompt rather than leaving the queue stuck.

Idle resume flushes the current transcript, waits up to 30 seconds for its tracked helpers,
and prepares the selected transcript before publishing a replacement snapshot. Failed storage,
unsettled helpers, and missing or malformed replay leave the current session selected. A successful
switch preserves attached clients, rebinds persistence and Lua/surface identity, and resets
session-local layout, accounting and cancellation state. Stored cursors and child-transcript
ownership survive both in-process and startup resume.

With `melchior` installed, a session can reach the other sessions in the same project. The
model calls one `agent` tool — `list`, `send`, `inbox`, `reply` — and magi's whole knowledge of
the layer is one file, `magi-cli/src/melchior.rs`, that spawns it and reads lines.

`:agents` shows the run's tree with each agent's current model, thinking setting, activity,
approval/input waits, and last admitted input against its context window and input limit.
`~` marks estimated tokens; unmeasured context and unreachable hosts are shown explicitly.
Reported session cost appears when available. Model names wrap rather than truncate. Details
refresh through read-only host queries while the panel is open, about every two seconds;
these queries neither subscribe as an approval screen nor call a model.

Opening another agent starts with a transcript snapshot at the tail, not a visible replay of
its history. New output follows the tail until you scroll up; PageUp/PageDown browse history,
and Shift+End returns to following new output.

Two walls decide who can be reached. The **project** wall is the filesystem: another checkout's
sessions are not refused, they are simply not there. The **instance** wall is the front door: a
main speaks for its instance, and its subagents are private, so `agent_talk` is `mains` by
default and `instance` or `project` when you mean otherwise.

A message carries a **sort**, and the sort decides what it may interrupt. `question`, `answer`,
`attention`, `trouble`, `handoff` and `report` wake an idle session; only `attention` and `trouble` may
reach one mid-turn. Anything arriving during a turn waits. Consecutive arrivals are
committed together at their queue boundary; they start one reply only when at least one
has a waking sort. Notes alone do not start a reply.

One main may ask another to **adopt** it. Consent is a person's: the request surfaces as a
prompt on the other side, and accepting hands down exactly the grants the parent already holds
— never more. Spawned children forward requests outside their grants to their parent's
permission policy and approval prompt. No blanket read, write, shell or network grant is added.

A session can also be *started* under another. `melchior fork` mints a name and a secret; the
harness spawns with what it was handed, and the session that comes up is a **child** — it writes
the note that makes the tree readable off the directory, it is inside the walls the policy draws,
and the session that minted its secret is the only one that can end it. melchior names; the
harness spawns, because a layer that started harnesses would have to know what one is.

`spawn` returns a child **agent id**. Check it with `agent` verb `status`, `who` set to that id,
or read `crew`. `task` instead takes the exact `id@message-id` handle returned by `ask` in
`about`. A tool-call refusal is not a child's failure report, and a report arriving while its
phase is `working` does not mean it has finished. Deferred finish/blocked wakes are canceled
if the agent resumes before delivery; a wake still asks the coordinator to check current status.
A headless coordinator reports `waiting`, not `finished`, while a result-collection wake is queued.

Every child turn hands in an end-of-turn report, including an empty answer, interruption,
provider failure or stopped worker. The harness preserves an explicitly submitted report,
adds final-answer and error information, and labels the outcome. This describes a turn, not
completion of delegated children. Workers must still use `agent` verb `report` for their
findings; they must not substitute `send`.

Report notifications queue behind active work. Before the parent's next model call, the harness
loads the stored report into the conversation and verifies its content digest. Submission
revisions deduplicate notifications, not equal text from separate turns. Automatic loading
carries at most 40,000 UTF-8 bytes per report; a longer report includes an explicit paged-read
instruction. Escape cancels the current turn only: reports arriving afterward wake it again,
and an interrupted report-handling turn gets one automatic retry without another notification.
Canceling that retry stops it; unread reports remain available for the next prompt. Interrupting
also releases outstanding permission and choice waits. Pending
receipts last for the live session; forced process termination or unavailable coordination
can still prevent delivery. These hooks do not reconstruct pending receipts after a restart.

## Layout

| Crate | Role |
|---|---|
| `magi-proto` | the wire contract: events, commands, envelope. No I/O |
| `magi-ipc` | Unix socket transport: magi's own CBOR frames, the family client, the runtime-directory rule, `SO_PEERCRED` |
| `magi-model` | the provider-neutral message model |
| `magi-core` | the turn loop, as an explicit state machine |
| `magi-tools` | what a tool is, and how one is dispatched to the tools program |
| `magi-lua` | the Lua VM, and the config API it offers `init.lua` |
| `magi-journal` | the transcript this process holds; balthasar is what stores it |
| `magi-host` | the session: the transcript, the socket, and the turns |
| `magi-tui` | rendering: theme, markdown, transcript, editor, status, footer |
| `magi-cli` | the UI process, and `melchior.rs` — everything magi knows of the agent layer |
| `magi-testkit` | fake harness and recordings |

## Development

`.make.lua` is the task interface; `make` on its own lists every recipe.

```sh
make test         # the suite
make gates        # the architectural gates
make clippy       # warnings denied
make verify       # all of it
```

| Gate | Rule |
|---|---|
| `gate-file-size` | no `.rs` over 800 lines |
| `gate-modules` | every `.rs` is reachable from its crate root |
| `gate-proto-size` | `magi-proto` under 4,000 lines |
| `gate-reachable` | no crate unreachable from the binary |
| `gate-cycles` | no two top-level modules depend on each other |
| `gate-hermetic` | the suite leaves behind no file and no process |
| `gate-wire` | one way of saying a thing crosses a boundary |
| `gate-one-store` | balthasar holds the history and magi keeps no copy |
| `gate-family` | the binary answers the family contract |
| `gate-sandbox` | one Lua VM, sandboxed, and a checkout cannot govern the session |
| `gate-lints` | every crate takes the workspace's denials and nothing takes them back |
| `gate-comments` | a comment describes the block; it is not a fifth of the code arguing with it |

`gate-cycles` is the one pi never built. It built *reachability* — and a cycle is maximally
reachable, so a reachability gate passes at 240,000 lines with the knot still in it. Ours had the
same blind spot, and this landed with an empty allowlist in all four repositories after finding
three cycles nobody had named.

`gate-hermetic` runs the suite under a `TMPDIR` of its own and asserts it is empty afterwards.
Every test used to tidy up on its last line — and `assert!` unwinds straight past a trailing
`remove_dir_all`, so a *failing* test always leaked. Three thousand six hundred directories had
collected before anything looked. It also counts what survives: a leaked process is found by its
working directory *or* by `$TMPDIR` appearing in its environment, because cargo runs a test binary
in the package directory and a child inherits that — the cwd question alone is blind to every
test that does not call `current_dir`.

`gate-sandbox` holds the count of Lua VMs at one. The sandbox's own tests prove the removals work;
what they cannot prove is that every VM went through the constructor that applies them, and a
second `Lua::full()` is a full standard library with no test anywhere going red.

`gate-modules` earns its place on its own: a file nobody declares is not a compile error, not a
warning and not run — it simply is not part of the crate. Two were found at once, each holding
tests that had silently not run since the commit that moved them.

`gate-comments` caps comment lines at 20% of the code in the same file. `SAFETY:` notes do not
count, from the `SAFETY:` line onward, and every file gets a floor of three lines whatever its
size. It flagged 175 files at first: the prose had grown into the argument for each decision, the
story of the bug behind it, and the case against alternatives nobody had proposed. What a comment
is for is the block underneath it.

The gates are not advisory. Every agent this one was measured against carries dead code nothing
reaches and a god file in the tens of thousands of lines — 6,549 in one, 34,875 in another. None
of that was decided; it arrived one reasonable commit at a time, which is the only way it ever
arrives, and the reason the limit has to be a script rather than an intention.

## Releases

`make build` builds against `x86_64-unknown-linux-musl` where the toolchain has that target,
which produces a genuine `static-pie` binary — no interpreter, no `NEEDED` entries, nothing to
install alongside it:

```
magi 0.1.0   2.15 MB
binary  target/x86_64-unknown-linux-musl/release/magi
size    2.15 MB   2,249,400 bytes
linking ✓ static   no runtime dependencies
```

The linkage line is read out of the ELF, not inferred from the flags: a glibc "static" build
still carries an INTERP and dies on a machine whose loader disagrees, and `ldd` reports it as
statically linked anyway.

The release profile is pinned rather than left to defaults — fat LTO and one codegen unit, for
size — and `make debug` is the build to iterate with.

Tagging a release builds the same binary on the runner, for `amd64` and `arm64`, and attaches
it. The toolchain is the flake's, so a release is built with the compiler the gates ran under.

## Nix

Each release tag publishes a static build for x86_64 and aarch64 Linux to the `ai-nerv` cache,
with `config/` under `share/magi/`.

```sh
cachix use ai-nerv
nix run github:ai-nerv/magi -- verbs
nix build github:ai-nerv/magi
```

The flake names the cache itself, so `--accept-flake-config` does the same as `cachix use`. In
another flake, take `inputs.magi.packages.${system}.default`, and configure the cache on the
consuming machine: an input's `nixConfig` is not inherited.

Here, `make nix-build` builds it into `result/`, and `make nix-check` checks the flake.

## Configuration

Everything magi knows about the outside world is Lua, and it all lives in `config/`:

| | |
|---|---|
| `config/apis/*.lua` | the wire protocols — how to talk to an endpoint |
| `config/providers.lua` | the catalog — which endpoints exist and what they offer |
| `config/tools.lua` | what the model may call, and how each tool is reached |
| `config/clients/*.lua` | a sibling's client library, if you name one — none ships, because `client` serves it |
| `config/init.lua` | your settings, and anything you want to add |

`make configs` copies them to `$XDG_CONFIG_HOME/magi/`, where magi reads them. The binary also
carries a copy, so a fresh install already speaks and already has a catalog — installing gives
you the real files to edit, it does not turn anything on that was off.

Layered, later winning by registration id:

```
compiled-in defaults  →  ~/.config/magi/apis/*.lua  →  providers.lua  →  init.lua  →  ./.magi.lua
```

A provider or a protocol declared twice replaces rather than appends, which is what makes both
an override and a loop over a directory of machines safe to re-run. A file that exists and does
not load is fatal: it expressed an intention that has not been carried out.

## Session environment

```
:source /some/bash/file
:source "$HOME/a file with spaces.sh"
:env OPENROUTER_API_KEY=your-key
:env PATH=/another/bin:$PATH
:env KEY="new value:${KEY}" OTHER='literal $KEY'
```

These commands update this session's environment overlay, not your parent shell or configuration
files. Future model requests, helpers, command tools and newly spawned subagents use it. Existing
subagents and already running commands are not changed. Set variables while the session is idle;
busy sessions refuse the update without changing anything. Model readiness updates immediately,
and a background catalog refresh follows. If your configured model was waiting for its missing key,
it becomes active once that key is set; an already active model is never switched. Without a
configured model, use `:model <name>` to select one.

Rune parses the source file without executing it. Only standalone top-level scalar
`export KEY=value` statements are imported, in file order. Other commands, unexported assignments,
functions, conditionals, pipelines and here-document contents are ignored. Exports requiring
command/process substitution, arithmetic, ANSI-C quoting or parameter operators are skipped and
counted. Malformed shell syntax refuses the whole file. Regular UTF-8 files are limited to 256 KiB.

`:env` accepts multiple assignments and expands `$KEY` and `${KEY}` against the session's current
environment before that command, as shell `export` does. Earlier export statements in a source
file affect later ones. Undefined variables become empty;
single quotes and backslash escapes preserve literal dollars. Colons, equals signs and quoted
spaces remain part of the value. Unsupported expansion in `:env` refuses the entire update.
Values are omitted from notices, command diagnostics, prompt history and durable session records.
They still exist in process memory and child environments, and tools explicitly printing them can
expose them. The OpenRouter variable is **`OPENROUTER_API_KEY`**, not `OPENROUTER_KEY`.

## Model discovery

`:model` opens the picker and refreshes provider catalogs through Melchior, bypassing its
discovery cache. The list updates without restarting the session or changing the active model.
Each model shows context and published input/output prices in USD per million tokens. Unknown
prices are `—`, not zero. OpenRouter entries also show `OR wk #N`, their position in its catalog
sorted by last week's token usage; this is popularity on OpenRouter, not a quality score.

Each model also shows advertised input/output modalities and native thinking levels.
OpenRouter supplies architecture and reasoning metadata; Ollama supplies `/api/show`
capabilities and `thinking.values` (boolean controls appear as `off,on`). Missing thinking
levels are shown as `available (levels unknown)`, not guessed from the model name. Missing
output metadata is `—`. These are provider capabilities, not a promise that Magi supports
every advertised modality; Magi currently accepts text and images. The separate `:thinking`
picker still uses Magi's neutral levels and the provider adapter's mappings.

The model picker is a table with one row per model: Model, Ctx, In$/M, Out$/M, ORwk,
Modality (input→output), and Thinking. Model names are never abbreviated and take priority
over metadata widths; the visible rows determine the name column's width. A terminal narrower
than a model name cannot show that entire name on one physical line.
Narrow terminals abbreviate modalities as T=text, I=image, A=audio, V=video, F=file;
thinking as 0=off/none, i=minimal, l=low, m=medium, h=high, xh=xhigh, x=max.
`on(?)` means thinking is available but its levels are unpublished. Long cells use ellipses.
Failed providers keep their previous choices with a warning; successful empty catalogs remove
models that are no longer available. Closing the picker does not reopen it when refresh
finishes. Magi and Melchior both need builds supporting this refresh protocol.

## Image inputs

Attach a raster file in a prompt with `@photo.png`, including in `magi -p "Inspect @photo.png"`.
For spaces, use `@"path with spaces/photo.png"`. Casper's `read` tool and MCP image results
also deliver native image attachments, not base64 prose. Supported inputs are PNG, JPEG,
GIF (first frame), WebP and BMP; SVG stays text. No browser is involved.

Casper validates and resizes raster inputs: at most 20 MiB source bytes, an 8192-pixel source
dimension limit, a 64 MiB decoder allocation limit, a 1536-pixel inline edge, and 128 KiB
encoded image bytes per attachment. Dense images can be reduced further to fit the byte limit.
Prompts and MCP results accept up to four images. Inline images are persisted with the
transcript and survive resume. Balthasar receives text projections plus estimated image-token
costs, not base64 in searchable prose; an elided tool image remains in the raw journal and can
be read again through the tool's recovery hint. The 50% input-context admission gate still applies.

Text-only models receive an explicit image-omission note. Switching to a vision model restores
the stored images when those entries are in the selected context. This implementation covers
image inputs, not audio/video inputs or image generation.

## Offline cache contract

From a NERV checkout, `oslo make test-cache-contract` runs the selected production
family against an offline Anthropic fixture. Magi reads a real file through
Casper, preserves signed reasoning and tool units through Melchior, and persists
the transcript through Balthasar. Captures check marker positions, stable frames,
normalized prefixes and supplied usage settlement; they do not measure real
provider cache hits or savings. The resulting receipt is explicitly scoped to
that scenario, not the full acceptance lane.

The Magi recipe takes an explicit absolute `--runner` path to the family
acceptance script. `oslo make test-session-client` checks the headless client's
argument parsing; `build-session-client` builds that protocol fixture. No sibling
Rust source dependency or installed configuration is added.

`oslo make test-cache-frames` checks transient `cache_frame` events and captured
Magi asks across ordinary growth, deferred-tool activation, changed rules/models,
selected stubs and summaries. `test-cache-routing` checks upstream preference and
release after transport failures. The observer keeps one fixed-size in-memory
snapshot per session, never prompt copies or durable fingerprints; events contain
only bounded reason codes. Restore/resume discards the snapshot and reports
`resume_incompatible` once because compatibility with prior requests is unknown.
These are provider-neutral observations before an ask, not evidence of delivery,
provider cache hits, or provider-owned dialect/image adaptation. Balthasar still
selects the layout, and fresh instructions and explicit routing choices still win.

Provider observations arrive separately from response deltas. Magi compares their
fixed-size policy identity only in session memory and publishes `provider_frame`
reason codes for dialect/policy changes and image adaptation. Missing or unknown
metadata clears compatibility, including across retries. These pre-send events
never release helpers awaiting a response, enter the transcript, or charge usage.
The headless test client accepts a validated inline `--image` with optional
`--image-type` for offline image-preservation fixtures.
Its `--resume <id>` waits for the initial attach snapshot, sends the production
resume command, and waits for the selected session snapshot before submitting a
prompt. A refused or timed-out resume fails without submitting that prompt.
The root cache contract exercises actual Balthasar replay and native Casper tool
history through resume, checking exact signed/image prefixes, unchanged durable
entries, and one explicit resume invalidation followed by ordinary stable growth.
An input-overflow fixture also checks actual Balthasar-selected read-result stubs,
the complete durable output, warning insertion/removal, and subsequent stable
masked prefixes. The client's `--successful-answers` waits past intermediate
error endings during recovery and fails on read errors or timeout.
It also captures shipped deferred-tool discovery, verified Balthasar user-rule
projection, and default-policy context pressure with whole-group drops and a
summary. Each main request is checked against the actual selected slots and full
durable history; invalidation reasons do not excuse arbitrary content changes.
Summary text stays on the assistant channel and retains the store's failed-call
guard. These are offline encoding and ownership checks, not measured cache hits.
