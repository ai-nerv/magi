# Experimental ACP adapter

`magi acp` uses the official schema1.12.0 with default features disabled and
bounded native transport. It starts ordinary Magi hosts; it does not implement a
second provider or tool loop. The experimental command is hidden from general
help. Its supported capability subset and verified local-editor scope are listed
below; unsupported optional methods are not advertised.

## Current protocol surface

| Method or content | Behavior | Focused coverage |
| --- | --- | --- |
| `initialize` | Negotiates wire v1 and advertises `loadSession`; other optional capabilities remain unadvertised | `acp::tests`, real stdio tests |
| `session/new` | Owned native host with separate cwd, controller key and versioned native-transcript-derived external ID | Native root/cleanup tests and history identity probes |
| `session/load` | Reserves the exact persisted native transcript, replays display updates before the response, then continues through the ordinary host | Five native loading cases: restart, refusal, concurrent ownership, ordinary-host exclusion and MCP replay/continuation |
| Session-local stdio MCP | Bounded literal configuration, native discovery/invocation/permissions and owned cleanup; other sessions and installed configuration unchanged | Bootstrap tests, generic stdio allow/deny/isolation fixtures and actual nested Casper read fixture |
| `session/prompt` | Text plus resource links; references preserved as native context without implicit reads/downloads | `native_acp_prompt_uses_native_turns_and_preserves_resource_references` |
| Stream/final | Native text, thinking and tool updates; final reply waits for its correlated native turn boundary after settlement/history flush | Native tool-round, tracked-wire and turn-state tests |
| `session/cancel` | Notification interrupts that session's pending native turn; completion reports `cancelled` | Concurrent-prompt and stalled-other-startup cancellation fixtures |
| `session/request_permission` replies | Correlated to native approval ID and owned session generation; only offered scopes, session lifetime | Permission unit tests and native allow/deny/cancel/out-of-order fixtures |
| Background child approvals | Forwarded through the owned native parent even after its foreground prompt completes | Real forked-child allow/deny/cancel-late-reply/disconnect fixtures |
| Automatic background turns/reports | Native arrivals/wakes, text/thinking and tool updates stream without an invented prompt response; cancellation/failure produces an update | Real child completion/follow-up and active background cancellation fixtures; foreground/background ownership tests |
| `session/set_config_option` | Native model selector; correlated host acceptance, complete configuration response and updates; session-local, no saved-default mutation | Settings unit tests and native model/refusal/busy/isolation fixtures |
| Compaction/rewind display | Negotiated completed compaction updates with replacement summaries; ordinary-message fallback without client support; rewinds retain earlier history | Display projection/negotiation tests and real native rewind fixture |
| General questions/surfaces | Interrupt and return an explicit unsupported-operation error; never implicitly approve | Native turn translation and unsupported-operation fixtures |
| Mode/client filesystem/terminal operations | Not implemented; no optional capability claims | Unknown methods return protocol errors |
| Image/audio/embedded-resource prompts | Rejected before native submission | Prompt admission tests |

The [ACP v1 prompt contract](https://agentclientprotocol.com/protocol/v1/prompt-turn)
and [content types](https://agentclientprotocol.com/protocol/v1/content) define the
wire surface. Text and resource links remain in scope; this checkpoint does not
narrow the finished baseline to text-only or empty-MCP sessions.

## Bounds and ownership

- JSON lines:16MiB; depth64;65536JSON values; duplicate keys rejected. Partial
  frame and output deadlines are30seconds. Initialized idle connections may wait.
  Both pipe-backed and socket-backed stdin/stdout use nonblocking native descriptor
  readiness. The adapter duplicates inherited descriptors with close-on-exec; it
  does not reopen socket descriptors through `/proc/self/fd` or use blocking stdin
  worker threads. Framing and deadlines are identical for both transport shapes.
- At most4pending client request IDs and2queued intake messages. A request keeps
  its ID lease until its response has been written. Pending duplicate IDs or
  admission overflow close the connection rather than produce ambiguous replies.
- At most4owned sessions, counting startups. Startup has a30second deadline and
  does not block cancellation or native events for an already-running session.
- New external IDs are `magi-v1-` plus lowercase hex of the exact native transcript
  key, obtained through a controller-only history-tail probe. Run IDs and child
  transcript keys are not conflated. Native keys are nonempty, control-free and
  at most512bytes; prompt/settings/cancellation IDs share a1032byte bound. The
  probe checks session/cursors/empty terminal page and expires after30seconds
  within the overall startup deadline. Missing/inconsistent identity refuses
  startup. No frontend identity index or second transcript store is written.
  This replaces process-local random IDs in the experimental adapter and supports
  cross-process loading; old process-local IDs are not migrated.
  Canonical decoding rejects odd/uppercase/non-hex encodings, invalid UTF8,
  controls and unknown versions. Recognized versioned IDs are validated in
  prompt/settings/cancellation before session lookup.
- Native readers own their framed reads without cancellation between prefix and
  body. The aggregate queue holds at most4native frames, each subject to the native
  16MiB frame bound. Partial frames and queue saturation expire after30seconds.
  Readers activate only after session registration and successful startup-response
  output. Discarding a paused reader does not fail other sessions.
- One pending prompt per session; at most64blocks and1MiB encoded prompt input.
  Concurrent prompts for that session are refused, not silently queued.
- Native display preserves user/workflow images without enabling image prompt
  admission. User asides, provider signatures and context masks remain private.
  `clientCapabilities.session.compaction: {}` enables structured completed
  compaction updates; missing/null capability uses ordinary agent text. Summary
  updates replace prior content, including an explicit empty replacement. Rewinds
  are display messages, not commands to erase earlier client history. These
  events never invent a pending prompt response or execute historical tools.
- Foreground requests carry checked native correlation IDs. Native admission
  atomically refuses a tracked prompt while busy; ordinary native FIFO semantics
  remain unchanged. Pending client admission and active native observation are
  separate bounded slots. Automatic turns never consume a client's pending reply.
  Native turn-owner boundaries, not a generic idle status, determine completion.
- Magi-local framed IPC is now version3 for these commands/events. Version0/1/2 peers
  are refused; matching Magi builds are required on both ends. The family wire,
  persisted history format and ACP wire version are independent and unchanged.
- Controller-only native history reads return at most128entries and8MiB per page,
  including a conservative metadata allowance. Sparse cursors are preserved;
  `next: null` marks completion. Wrong session/cursor, busy host and an entry that
  cannot fit are explicit errors, not shortened or skipped history. Pages use the
  exact framed writer rather than the terminal's display-truncation path.
  Native private fields are excluded from ACP replay output.
- The adapter's bounded page reader checks native session, transcript and fixed
  end cursor on every page. Full reads require an expected transcript; only an
  empty-tail identity probe may discover one. Sparse cursors must strictly
  increase within the requested range. Continuation must equal the final returned
  cursor; terminal pages must reach the requested end. Invalid pages do not
  advance state. Each read has a30second deadline,128entry/8MiB encoded-byte cap
  and a separate connection. Completed readers perform no further request.
  This reader serves startup identity probes and bounded startup replay for both
  new and loaded sessions.
- Persistent loading requires the generic store-scoped writer capability. The
  ordinary host reserves before lookup/replay and keeps that exact connection
  through startup and serving. Missing, busy or incompatible stores refuse loading;
  another owner is never displaced. Lost owned transports never replay writes.
  At a flush boundary, recovery acquires a fresh reservation and checks the run
  binding and durable history against acknowledged and uncertain records before
  sending queued entries. Busy ownership or incompatible history preserves the
  queue and keeps recording stopped.
  Oversized native raw records are reconstructed under that reservation with a
  16MiB record cap,256fragment cap and30second total record deadline. Invalid or
  incomplete fragments never advance history or acknowledge queued entries.
  The supplied cwd selects the native store; an ID alone does not select another
  workspace. Loading uses the private bootstrap and shares the30second startup
  deadline. Fresh session-local MCP settings replace the old session's settings;
  historical permissions are not restored and historical tools are not executed.
- Startup replay projects one native journal entry at a time without executing
  tools or recounting usage. A global one-frame queue serves at most4startups;
  each producer awaits a successful full-frame output receipt before sending its
  next update. Queue admission alone is not delivery. Enqueue and receipt share
  a30second deadline inside the overall startup deadline. Removed generations and
  expired/abandoned queued frames are discarded. Abandonment during an output
  write fails the connection, so a partial frame cannot precede another response.
  Replay must reach its exact terminal history page and receive every output
  acknowledgement before startup can succeed. No whole-transcript display vector
  or frontend transcript store is built.
- Native admission has a75second deadline; cancellation has30seconds. Provider
  and spend limits remain native policy, not adapter-selected replacements.
- Native model options come from the snapshot/catalog, not an adapter catalog.
  Catalog limits are4096entries,512-byte names,4096-byte requirement descriptions
  and1MiB encoded configuration. Requests are limited to16KiB. An unconfigured
  native host advertises no model selector rather than an invented default.
  One pending change per session keeps its request ID until the response write;
  prompts and other settings are refused while it is pending. Correlation and
  a30second deadline are checked on receipt as well as by the dispatcher timer.
  Native busy/unavailable refusals do not become successful acknowledgements.
  Ordinary terminal model selection retains its saved-default behavior; ACP
  selection does not write `chosen.json`. Modes/thinking remain unadvertised.
- At most16pending permission requests,16native scopes per request,64KiB native
  action/scope/advice payload and8KiB permission result. Requests expire after
  240seconds with denial. Approvals belong to the owned session/generation and
  may outlive a foreground prompt. Cancellation retires their authority before
  completing the turn; idle-session cancellation denies outstanding approvals.
  A bounded64-ID tombstone queue consumes late replies without granting.
  Unknown/duplicate reply IDs and unoffered choices close the connection.
- Permission options preserve native scopes and the native terminal's session-only
  lifetime. Remembered choices explicitly say "this session only"; the adapter
  never requests a persistent grant. Native approval IDs are distinct from model
  tool-call IDs, so approvals are represented as separate authorization operations
  with disjoint tool-call ID namespaces, not guessed associations by tool name.
- MCP bootstrap:16servers,64arguments/server,64environment entries/server,64KiB
  total. Commands and cwd must be absolute. Additional directories and unsupported
  transports/effect-bearing fields are refused.
- Bootstrap uses an owned0600anonymous file, never a named secret file. The child
  consumes it before config loading and replaces stdin with `/dev/null`. Failure
  to create an anonymous file refuses startup.
- Child startup sets `CASPER_JAIL=1` and removes inherited session/socket authority
  redirects. Native controller keys stay private and never enter ACP frames.
- Disconnect cancels pending startups and interrupts/drains owned active turns
  under a shared5second budget before native host termination, and denies pending
  idle-session approvals. Active work is interrupted before a denial could resume
  a follow-up model call. Each owned host
  gets5seconds after TERM before forced termination. Unrelated hosts are not
  stopped. No universal kernel-level cleanup deadline is claimed.
- Parent-death protection uses KILL during config loading, then TERM after the
  native signal handler is installed. A fixed private readiness marker gates
  controller attachment; native stdout is then redirected to `/dev/null`. Owned
  hosts interrupt and settle native work under2seconds before normal shutdown.
  Separate startup-crash and active-generation SIGKILL fixtures require cleanup.
  Ordinary headless/fork lifetime regressions also pass.

## Verification

Use `oslo make test-acp-host`, `oslo make test-acp`,
`oslo make test-acp-sessions`, `oslo make test-acp-loading`, `oslo make test-headless-lifecycle`,
`oslo make test-lifecycle` and `oslo make test-skills-submission`. Native fixtures require
selected Casper/Melchior/Balthasar binaries and tools; `MAGI_REQUIRE_LIVE=1` makes
missing prerequisites fail rather than skip. Tests use disposable configuration
and a synthetic-only model boundary while retaining real native host, coordinator,
store and tool execution. No installed catalog or paid provider is used.

The final-image family verification on2026-10-29 passes8,132records with no failures
or leaked processes; thirteen opt-in visual/editor/measurement records are skipped
in that invocation. The explicit editor gate separately passes all seven actual
Neovim0.12.4/Agentic scenarios with zero ignored cases: streaming and recovery,
permission controls, native running-tool cancellation, model selection/refusal,
and persistent loading in a fresh editor. Each adapter's actual executable is
checked through `/proc/<pid>/exe`. Native headless and80x24/160x28 workflow checks
and all fifteen deterministic family acceptance scenarios also pass.

Retained receipts under the family candidate
`target/roadmap/11/family-candidate.abuXbcuD`:

- `target/family/run-VE6tVqb8/summary.json`: full native family verification.
- `target/acceptance/run-VvDpo1T4/summary.json`: deterministic acceptance.
- `target/roadmap/11/selected-editor-workflows.FGs0csXN`: seven actual-editor
  cases and selected native child-event checks.
- `target/roadmap/11/selected-magi-test-skills-live.lcvdI3mE`: retained headless
  and terminal workflow evidence.

These establish the tested native/editor behavior, not universal editor
compatibility, manual font/color approval, real-provider consent or deployment.
The full-family invocation does not silently promote its skipped editor cases;
the separate explicit editor receipt supplies that evidence.

## Historical implementation checkpoints

The checkpoints below retain their original evidence scope. Their pending-work
statements describe those earlier revisions, not the current verification above.

The persistent-loading checkpoint passes39native ACP sessions,442host-library,
3host-resume,18journal,2Lua identity and6CLI-resume tests, Clippy,11Magi gates and
the root family-runner regression suite. Receipt in the isolated family candidate:
`target/roadmap/11/acp-loading-integrated.log` (exit0). This is not a new full-family
receipt or completed editor-loading acceptance. Earlier checkpoints below describe
the capabilities and remaining work at their respective times.

The subsequent selected-artifact editor checkpoint passes all39native ACP cases
and seven actual Neovim/Agentic scenarios, Clippy and11gates. Its new loading case
exits the first editor and adapter, verifies owned-process cleanup, then uses
Agentic's `restore_session_by_id` in a fresh terminal-backed editor. It checks the
same persisted session, a distinct adapter PID, the selected `/proc/<pid>/exe`,
rendered replay before continuation, zero provider calls during replay, and native
history in the continued provider request. Receipts and32-row terminal grids:
`target/roadmap/11/acp-loading-editor-first/loading-terminal/{seed,load}` in the
isolated candidate. The pinned client remains unchanged. Run the explicit
`oslo make test-acp-editor` gate with the selected inputs listed by the recipe;
`test-acp-editor-smoke` remains its compatible underlying target. This proves the
recorded editor scenarios, not universal editor compatibility or full Phase10
acceptance. The seven opt-in cases remain ignored in ordinary family tests.

The MCP/child-approval checkpoint passed40ACP unit,5stdio,27native-session and
21ordinary child/headless/fork checks, release check/Clippy and11architecture gates.
Receipt: `target/roadmap/10/child-approval-shell-floor.log` in the family checkout.
Earlier receipts remain, including the reproduced idle-parent approval failure
and the fixture's missing-shell diagnostic. No jail policy or positive action
assertion was weakened. Native tests require real
tool output, a fresh approval after allow-once, correctly routed out-of-order
replies across sessions, zero owned fixture processes and runtime cleanup. Refused
action markers are checked while the disposable workspace still exists.

The background-boundary checkpoint passes162 focused host/proto/IPC checks,
44ACP unit,5stdio,28native-session and21ordinary lifetime checks, plus71ordinary
host lifecycle/roundtrip checks and23native workflow-submission cases. Release
check/Clippy and all11architecture gates pass. Receipt:
`target/roadmap/10/background-boundary-modular.log`. The child fixture verifies
an automatic parent response after foreground completion and cancellation without
a second prompt reply. Native receipt tests cover busy refusal, preparation
cancellation, exact turn-owner settlement and pending foreground/background races.
The cancellation fixture uses an exec handoff, matching the existing direct
provider fixtures; this is not arbitrary provider-descendant supervision proof.
Roundtrip fixtures assert the new live boundary explicitly; recorded replay stays
unchanged. All failed receipts remain, and no resource or cleanup assertion was
waived.

The model checkpoint passes163 focused host/proto/IPC checks,52ACP unit,5stdio,
31native-session,21ordinary lifetime,71host lifecycle/roundtrip and23native
workflow-submission cases, plus release check/Clippy and all11architecture gates.
Receipt: `target/roadmap/10/model-native-deadline.log`. Initial size-gate failure is
retained in `model-native-first.log`; stale protocol header documentation was
shortened/corrected without raising the gate or removing tests. Native fixtures
verify the selected model in actual provider requests, unchanged sibling sessions,
unavailable-model refusal, busy refusal with live cancellation, reusable completed
request IDs, unchanged directory defaults and zero owned process/runtime leaks.
The additional virtual-time test rejects expired receipts even when the event
branch wins dispatch selection before the timeout branch.
The selector uses the stable
[session configuration contract](https://agentclientprotocol.com/protocol/v1/session-config-options),
not the experimental model-setting method.

The native-history checkpoint passes173focused host/proto/IPC checks,52ACP unit,
5stdio,31native-session,21ordinary lifetime,76host lifecycle/roundtrip and23native
workflow-submission cases, plus release check/Clippy and all11architecture gates.
Receipt: `target/roadmap/10/history-native-bounded.log`. Five paging tests cover
count/encoded-byte limits, sparse cursors, explicit completion, invalid/busy/changed
boundaries and no read-side publication or usage mutation. A real socket fixture
checks controller authority and two-page fidelity; codec tests preserve typed
success/refusal and reject previous native wire versions. Closed and partially
written display sockets now return write errors instead of false success. The
first fixture-closure compilation failure remains in history-native-first.log.
These checks do not establish persistent ACP load or editor acceptance.

The display/identity checkpoint passes58ACP unit,5stdio,32native-session and
21ordinary lifetime checks, release check/Clippy and all11architecture gates.
Receipt: `target/roadmap/10/display-identity-first.log`. The real native fixture
checks the external ID against the controller's actual transcript key, an idle
rewind, a background image prompt with private context, no unsolicited response,
continued foreground prompting and owned cleanup. Unit fixtures cover capability
negotiation, native historical display projection, stable replacement summaries,
exact identity boundaries, length limits and a stalled probe deadline. The
earlier display-only receipt is display-native-first.log. No gate failed in
either run. Historical projection tests are not persistent-load acceptance.
Compaction follows the official
[v1 session-update contract](https://agentclientprotocol.com/protocol/v1/prompt-turn#session-compaction).

The page-reader checkpoint passes63ACP unit,5stdio,32native-session and21ordinary
lifetime checks, release check/Clippy and all11architecture gates. Receipt:
`target/roadmap/10/history-reader-first.log`. A framed fixture returns300sparse
entries over3pages and proves exact completion without an extra request. Further
tests reject mixed identity, repeated/backward/out-of-range cursors, premature or
nonadvancing endings and excessive entry/encoded-byte sizes without changing
reader state. Exact8MiB and escaped-text overflow boundaries pass. These reader
fixtures do not establish loaded-session ownership, replay delivery or acceptance.

The replay-delivery checkpoint passes75ACP unit,5stdio,32native-session,
21ordinary lifetime,173host/proto/IPC,76host lifecycle and23workflow submission
checks, release check/Clippy and all11architecture gates. Counts are recipe
executions, not distinct whole-project tests. Receipt:
`target/roadmap/10/replay-delivery-barrier-fixed.log`. Fixtures cover130sparse
entries over2pages, last-frame acknowledgements, bounded queue/receipt/write
deadlines, identity/privacy/schema checks, stale generations and paused readers.
The retained replay-delivery-abandon-repro.log reproduces abandoned partial
output; replay-delivery-cancel-fixed.log verifies that repair but exposed a separate
display-fixture ordering race. The final fixture uses an ordered native refresh
notification, not sleeps or prompt retries, to prove the adapter consumed the
background completion before submitting another foreground prompt. These changes
do not implement session/load or establish actual-editor acceptance.

At that checkpoint, persistent load/replay integration, complete native regressions
and family gates, and actual-editor acceptance were still required. ACP allows session updates
outside foreground turns; see the
[v1 session-update contract](https://agentclientprotocol.com/protocol/v1/prompt-turn#session-updates).
No full Phase10 completion claim follows from these focused tests.

## Isolated editor smoke

`oslo make test-acp-editor-smoke` runs the selected Neovim with a pinned
[Agentic client](https://github.com/carlos-algms/agentic.nvim/tree/f0bef086b3ed6121d9a18a5a820d071c69f2f0f5),
without loading user init/plugins or changing installed configuration. It requires
absolute `MAGI_TEST_EDITOR`, `MAGI_TEST_PTY` (util-linux `script` with direct argv
support) and `MAGI_TEST_EDITOR_CLIENT` paths and a fresh
`MAGI_EDITOR_EVIDENCE` output directory, plus the selected native family variables
`CASPER_TEST_BINARY`, `BALTHASAR_TEST_BINARY`, `MELCHIOR_TEST_BINARY` and
`CASPER_TEST_TOOLS`. The client checkout must match the pinned commit with no
working-tree changes. No package manager, Node runtime or network fetch is invoked
by the recipe. Missing inputs fail this explicit gate. Its seven integration tests
are ignored in ordinary Cargo test invocation because those external inputs must be
selected separately.

Neovim0.12.4 passed the first socket-compatible run, retained in
`target/roadmap/10/editor-smoke-socket.log` and `editor-smoke-socket/`. The actual
plugin chat buffer displays a controlled streaming prefix before completion,
then accepts a second prompt in the same native session. The provider sees the
first prompt in its native history. Buffer text, editor version and client/session
identity are retained; cleanup asserts zero owned processes and no saved model
default mutation. The first failed run, editor-smoke-first.log, exposed libuv's
socket-backed stdio: reopening it through `/proc/self/fd` failed with ENXIO before
initialization. The descriptor fix also passes75ACP unit,7real stdio,32native
session and21ordinary lifetime checks, release check/Clippy and all11gates.

The terminal extension passes in `target/roadmap/10/editor-terminal-cancel.log`
and `editor-terminal-cancel/{buffer,terminal}/`. Both actual-client scenarios
complete two turns, cancel a held third turn through `agentic.stop_generation()`,
and complete a fourth turn in the same native session. The third provider is
never released, its suffix never appears, and no provider overlap is observed.
The terminal scenario attaches Neovim's real TUI through selected util-linux
`script` (verified with2.42.3). Four120×32 rendered cell grids record streaming,
completion, cancellation and recovery; raw terminal output is retained too.
These grids have been inspected for the expected visible text. They are not a
font/color screenshot or a claim of complete editor UX acceptance.

Both scenarios have a60-second overall deadline and actively capped1MiB stdout
and stderr collection. Failure artifacts retain bounded prefixes before cleanup
assertions; timeout and early exit explicitly kill/reap the editor. The
`test-acp-editor-harness` recipe checks exact output bounds, overflow before EOF,
artifact bounds, stalled-process timeout and early exit. Parallel scenarios use
distinct synthetic provider directories. The failed shared-directory run is
retained in editor-terminal-first.log, not removed or hidden by serial execution.
The passing extension also repeats75ACPunit/7stdio/32native/21ordinary lifetime
checks, release check/Clippy and all11gates; selected runtime binaries are unchanged.

The permission extension passes in `editor-permissions-native-path.log`:
both actual-buffer and real-TUI scenarios allow one native read, require a fresh
approval for the same read, reject that read, and reject a shell action. Keyboard
navigation and Enter use the client's real permission mappings, including the
fifth option that has no numeric shortcut. The approved file content reaches the
editor and native provider; replacement content is never read after rejection,
and the rejected shell's target file never exists. Four rendered permission
screens per terminal run are retained. The fixture uses canonical utility and
shell directories already used by native sandbox fixtures, not sandbox bypasses.
The initial missing-utility failure remains in editor-permissions-first.log.

The new fifth scenario exercises the actual Neovim model picker, busy refusal,
idle selection and unavailable-model refusal. All five actual-editor scenarios
pass in `editor-cursor-first.log` (5479). Fixture repairs send carriage return,
acknowledge fresh hit-enter notifications, consume output through each rendered
picker and wait for the client's asynchronous header refresh. The external client,
runtime and behavior assertions remain unchanged; all earlier failures remain.
The model receipt records both refusals, fake/two selection, three completions
and exactly three native provider calls. Selection itself makes no provider call
or saved-default write. Inspected terminal grids show the selected fake/two header
and successful post-refusal recovery; owned cleanup passes. Five supervision
tests, check/Clippy and11gates pass with354source/14baseline/6runtime hashes checked.
Evidence is in `target/roadmap/10/editor-model-cursor-20261027/` from the root.
Full visual and persistent-load acceptance remain explicitly false.

The sixth actual-editor scenario cancels an already-running native shell tool.
Real permission keymaps allow it once; a FIFO reader's PID/start-time identity
proves execution before the public editor stop action. The native cancellation
result appears in the buffer, the reader is reaped, no final effect or automatic
provider followup occurs, and an explicit next prompt succeeds in the same session.
`editor-tool-first.log` (93146) passes34native ACP cases,5supervision tests and
all6editor scenarios plus check/Clippy/11gates. `editor-tool-visible.log` (6234)
passes6editor cases with the cancellation-result assertion.357source/14baseline/
6editor-runtime identities match. Retained receipts, raw PTY output and120x32
screens are under `target/roadmap/10/editor-tool-visible-20261027/`; the cancellation
and recovery grids were inspected. No production or pinned-client source changed.

Independent `editor-controls-native-regression.log` passes `test-acp`,
`test-acp-sessions`, `test-acp-host`, `test-child-permissions`, `test-resume`,
`test-turn-boundaries`, `test-headless-lifecycle` and `test-skills-submission`.
Native runtime identities are unchanged. These checks do not rerun or waive the
failed editor-model case.

Full native-family verification subsequently passes in
`target/family/run-bWQF2mvz/summary.json` (root workspace), with all32native ACP
session cases required and zero leaked processes. The hermetic runner supplies
the selected Melchior binary explicitly; fixtures reject stale inherited
selection and omitted/ignored/failed/partial ACP results. Its five opt-in editor
cases remain ignored by ordinary member verification, not accepted or waived.
The full run preserves352tracked source hashes and14retained baseline hashes;
`target/roadmap/10/family-integration-verify.log` records that run.

The later `tool-lifecycle-first.log` passes34native ACP session cases plus
check/Clippy and11gates. Added cancel/disconnect cases first approve a real shell
tool and observe its process waiting on a FIFO; teardown must reap that reader,
prevent the post-barrier action and avoid an automatic model followup. The
cancellation case then completes another prompt without provider overlap.
Both are mandatory family-runner cases. This test-only extension does not change
the runtime or resolve the pending editor/model and persistent-load decisions.

An identity check then exposed that the native ACP fixture bypassed the selected
Magi binary. Earlier native ACP receipts used Cargo's executable, not necessarily
the retained family artifact. The fixture now uses the existing selection helper,
asserts the adapter's actual /proc executable and uses that identity for native
host checks. `tool-selected-first.log` passes all34cases against explicitly
selected Magi9c897a8c with four retained binary hashes checked before and after.
The corrected full-family rerun passes Magi/Casper/Melchior, but fails the
unrelated Balthasar bounded-output fixture with a broken input pipe; receipt
run-0JxwGfXE remains FAIL. That fixture now consumes stdin before emitting output,
without changing its output-cap assertion or production behavior.
`tool-reaping-context.log` passes34ACP cases and Balthasar's full test recipe.
Tool teardown additionally checks PID/start-time disappearance, not merely an
empty command line. The final full-family rerun passes in
`target/family/run-7lF2KxOb/summary.json`:34required selected-native ACP cases,
all four members and zero leaked processes.354source hashes and14retained
baselines match; the four selected runtime digests match run-bWQF2mvz. Its five
opt-in editor skips do not establish full editor acceptance. Failed receipts
remain unchanged; no runtime behavior or sandbox policy changed in these repairs.

The earlier selected-image `test-acp-editor-smoke` passed six actual-editor cases:
buffer/TUI streaming and recovery, buffer/TUI allow/deny, model selection/refusal,
and running-tool cancellation. It honors `MAGI_TEST_BINARY`, verifies the actual
adapter's `/proc/<pid>/exe`, and retains that identity in each receipt. All six
cases passed against retained family run-Rpyo7E1C with owned cleanup verified;
the running-tool case also proves process-instance reaping and no final effect.
Evidence: root `target/roadmap/11/editor-selected-evidence.sha256` and the isolated
candidate's `target/roadmap/11/editor-selected-final/`.

That earlier six-case receipt did not prove persistent writer ownership/loading.
The current `test-acp-editor` recipe runs all seven scenarios, including fresh
editor loading, with retained replay/continuation and process-identity evidence.
General font/color visual approval remains separate from rendered text-grid and
native lifecycle acceptance.
