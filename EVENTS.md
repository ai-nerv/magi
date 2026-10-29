# Local operational events

## Native lineage binding

Native bootstrap revision2 carries an immutable run/agent/turn/ask binding in a
512byte packet. Event envelopes remain revision1 with their4096byte cap. A bound
run and agent must occur together; turn or ask requires that pair. Strict decoding
rejects malformed, unknown and duplicate fields. Parent PID/effective-UID and
descriptor admission remain unchanged. No environment or model arguments select
these identities. Capability negotiation requires bootstrap2; older peers keep
their useful-work path without telemetry and increment the unobserved count.

The host creates opaque run/agent IDs at collection startup. Producers apply
immutable bindings after local attribution; the collector verifies them before
deduplication or usage accounting. Unbound source-owned IDs remain intact.
`lineage-native-fixture.log` passes483 reported test records in44 reports with
zero failures or ignored tests, including fully bound selected native launches.
Repeated lanes are not distinct tests.

The task-context implementation scopes admitted turn IDs and individual
broker ask IDs to their futures. Streaming launch captures the binding before
blocking capability negotiation. Explicit local facts retain their own IDs, and
cancelled, nested or concurrent scopes must not contaminate another invocation.
Immutable process fields are applied before filling absent task context, so an
ambient ask cannot attach to a different fixed turn. `turn-lineage-bound.log`
passes1003 reported records in35reports, including nested/interleaved futures,
cancellation/unwind, detached-task isolation, active-versus-queued admission,
a real-session tool authorization sharing the admitted turn, and selected native
broker/CLI lifecycle regressions. All four owners pass Clippy and architectural
gates. Durable daemons receive run/agent-only bootstrap; single-call Casper runs
capture the caller's current turn/ask binding.

## Casper request attribution

An actually bootstrapped Casper daemon advertises `request_context: 1` on its
listening announcement only with root-only lineage and a validated parent PID.
Magi sends a fixed-shape `event_context` beside the ordinary call/key, never inside
tool arguments, only when that capability is present. Its schema1 fields are
canonical opaque run/agent/turn/ask IDs; serialized metadata is at most256bytes.
Old peers receive the unchanged request shape. Missing current host context sends
no metadata, and unsupported peers do not query the context callback.

Casper checks the normal daemon key first, then the original bootstrap parent
PID/effective UID and exact run/agent pair. Invalid optional context clears request
attribution without refusing, replaying or changing useful work. Source-owned tool
and MCP connection traces capture immutable context at creation; emission never
consults ambient request state. A connection trace that outlives its opening call
retains that call's attribution rather than adopting a later caller's identity.

`daemon-lineage-typed.log` passes427 reported records in36reports, zero failed or
ignored, with Casper/Magi check, Clippy, gates and candidate builds. The stronger
`daemon-lineage-reuse-fixture.log` passes301 reported records in32reports and Magi
check/Clippy/gates. It covers actual reused-connection context clearing, distinct
and concurrent turns, wrong key/root/peer, oversized metadata and useful results.
The isolated supplier wire fixture checks capability gating, current/absent host
context and unchanged model arguments. These counts include repeated lanes.
Memory request attribution is described under Verification below. Queued
surface/judging helpers, spawned-agent ancestry, complete containment, overhead
and current full-family/editor acceptance remain outstanding.

## Session helper attribution

Session-owned helper tasks capture the initiating turn before moving to their
runtime, without inheriting the parent's provider ask. Each broker invocation
continues to allocate its own ask identity. Between-turn helpers capture the active
admission when registered, before waiting for the turn to end; they never consult
a later admission for attribution. Registration with no admitted turn explicitly
clears unrelated ambient context. Recovered job history is not a claim about the
historical turn that originally created the job.

`helper-lineage-owner-tests.log` passes601reported records in16reports with zero
failed/ignored, plus check, Clippy and11architectural gates. The tests include
16helpers moving across runtimes,32decoded events with distinct asks, the actual
between-turn/Scribe wire path after admission changes, all11native layout/helper
cases,521host unit tests and native host/CLI resume suites. Arbitrary detached
tasks do not inherit context. Source metadata on surface/judging question queues
and spawned-process ancestry remain separate work.

Phase11 initial policy is approved; this is the first owner-local implementation,
not complete instrumentation, collection or export.

`magi_host::events` observes native prompt, permission-request, policy, authorization and
client transport activity. Trusted configuration can opt in with `magi.events = true`;
unset or false stays disabled. Non-booleans are refused, nil clears an earlier
assignment, and an untrusted project file cannot set this option.
`emit` is disabled until an owner explicitly calls `enable_local`; the disabled
path does not evaluate its payload closure, allocate a queue or obtain entropy.
Only one receiver can be created per process lifetime. Dropping it closes delivery;
later emissions count closed-channel loss rather than reopening a hidden sink.
There are no files written, workers, sockets, browser actions or network exporters
on the default runtime path.

## Normal session activation

Collection starts before normal session siblings and remains owned outside the
async runtime. Runtime shutdown, tool-daemon and static Balthasar cleanup and any remaining opening
child cleanup precede collector shutdown. Early returns and reported print failures
unwind these owners rather than exiting around them. Native launch Commands are
consumed once; input and producer lifetime stay with the child until actual reaping.

Normal Melchior serve/ask, Balthasar serve and Casper run/serve launches negotiate `event-capabilities`
with their configured program. Independent owners answer fixed family JSON before
loading configuration. Queries have null stdin, discarded stderr, PATH-only
environment, a512byte output cap, a one-second deadline and group cleanup before
reaping the leader. Capability evidence is not executable attestation. Disabled
sessions never query capabilities. Unsupported siblings still launch normally;
fallback attempts increment saturating `unobserved_launches`, not a zero-loss claim.

Native read-only Inspect keeps its existing CBOR envelope and detail fields, adding
`events`: state disabled/unavailable/collecting, an optional bounded summary and a
nonzero unobserved-launch count. Existing detail readers remain compatible. No
paths, prompts, credentials or producer labels enter this status. This is a local
view, not a billing or complete-run receipt.

Candidate `normal-events-owned.log` passes77 reported results, including explicit
native bootstrap, three normal launch scenarios, selected executable identity,
older-sibling fallback, no disabled queries, observed loss accounting, child reaping,
headless/fork lifetime and print exit contracts. Magi check/Clippy/gates pass; the
unchanged sibling code passed all three owners' Clippy/gates in normal-events-wire.
Provider asks negotiate on a blocking worker, then prepare and spawn on the caller's
runtime thread. Melchior's parent-death signal remains tied to a durable thread.
Cancellation during negotiation cannot launch a provider or register its producer.
Async streams retain native child ownership through kill, reap and event retirement.
Candidate `provider-events-native.log` passes83 reported results: broker regressions,
selected-native interoperability, normal sessions and headless/fork lifetime checks.
The provider integration record covers disabled/enabled answers, older-owner
fallback, negotiation-worker expiry and cancellation before/after native launch.
Check, Clippy and all11 gates pass. Earlier failing receipts remain retained.

Normal Casper calls retain admission-before-request and no replay after a sent or
ambiguous operation. Daemons stay attached to the durable keeper thread; shutdown
reaps them before producer retirement and removes only the recorded socket inode.
Per-call cancellation closes the peer; Casper checks that disconnect while draining
subprocess output and refuses subsequent launches in that call's scope. Ordinary
headless shutdown now interrupts and settles the turn before memory/runtime teardown.

`supplier-cancel-repair.log` verifies native run/serve cancellation, daemon reuse,
disabled/legacy behavior, positive admission refusal and producer retirement.
`supplier-headless-settlement.log` verifies real selected four-owner sessions with
idle/active tools and collection disabled/enabled, synthetic loopback results,
actual host-PID reaping and owned-socket cleanup. The shell's namespace PID is
mapped through Casper's descendant tree before cleanup is asserted. Earlier weak
green and failing cancellation/shutdown receipts remain retained.

Complete containment, inherited lineage, measured overhead and current
full-family/editor acceptance remain pending; focused receipts are not phase completion.

## Local collector component

Selected-native verification now includes independent Melchior and Balthasar
receivers/forwarders. `test-events-native` checks actual executable identity,
stdin restoration, Melchior loopback results and self-exec, bind failure,
Balthasar key admission and observe/layout, signal-mask inheritance and cleanup,
failed startup, and real child reaping before producer retirement. The passing
`sibling-bootstrap-observed.log` is component/interoperability evidence, not
normal-launch activation or full-family acceptance. The later normal session
checkpoints above cover Melchior serve/ask and Balthasar serve negotiation, not
Casper normal launch integration or cross-owner lineage.

`events::collection::Collector` takes the existing sole local receiver. Explicit
registration creates private nonblocking close-on-exec Unix datagram pairs, with
at most32 concurrent producers including local Magi. `open` mints a fresh opaque
producer identity and binds the channel to its expected owner. Production callers
cannot rebind a previous identity. It has no filesystem listener or network endpoint.

Each poll consumes at most one frame per producer, using a4096byte receive buffer.
Truncated frames and ancillary data are rejected. Send/receive kernel buffers are
requested at16KiB; setup refuses effective buffers over64KiB. Send is one attempt,
with saturating full/unavailable/invalid counters. `Sent::Queued` describes kernel
acceptance, not collector receipt. No second userspace event queue is added.

Independent typed consumer schemas validate all four owners before admission.
Unknown fields, mismatched owners/producers and malformed envelopes are rejected
before deduplication. The collector retains at most4096 producer/sequence keys
for60seconds, with explicit age/capacity eviction counts. Replays after eviction
can be accepted again; there is no exactly-once guarantee.

The usage view retains at most1024 producer/ask identities for60seconds. Higher
revisions replace cumulative snapshots rather than adding them. Unknown values
stay unknown, stale revisions are ignored, conflicting equal revisions are
counted, and unrelated producers do not share a usage record. Missing ask lineage
is counted rather than inferred from request IDs. This view is not a billing ledger.

Producer status uses a separate strict, owner-bound envelope with a checked
revision, successfully sent event count, queue/transport loss and final flag.
Reports have at most one attempt per second, including failed and final attempts;
there is no recursive event, retry or shutdown wait for an acknowledgment. A final
report confirms transport only when its event count matches the received valid
frames and there were no invalid frames or transport faults. Duplicate event frames
count for delivery reconciliation, not accepted facts or cumulative usage. Missing
final status is unknown loss, never zero. Known reported loss remains incomplete
even when the final report is confirmed.

`Worker` owns a joined collector thread, a bounded registration-control queue and
atomic readout. Admitted plus pending channels share the32producer limit. A queued
registration is not acknowledgment of admission; refusal closes only its channel
and returns its lease. Stop wakes and joins the worker, without waiting for remote
producers. Shutdown overruns beyond one second and worker panic remain visible;
the worker is never detached. Receiver closure explicitly shuts down its socket
even if a spawning process temporarily holds a duplicate descriptor. Sender drop
does not shut down the transferred endpoint.

Each `Opened` returns a sender and a separate `Lifetime` token. The root retains
that token until the native owner has exited, including cancellation/reaping.
Dropping the token is nonblocking. Polling drains at most one frame per producer
until empty or a one-second retirement deadline. A final report also initiates
retirement; queued post-final traffic is rejected before an empty channel retires.
Deadline abandonment is unknown loss. Retirement releases the channel and slot,
preserving saturating totals of completed/unknown/loss-bearing producers and their
last reported queue/transport losses. No retired identity map is retained. Old
tokens cannot close replacement channels; production opens mint new identities.
Active delivery records disappear at retirement; dedup and usage keep their
existing independent count/age bounds.

Readout is fixed-size counts and completeness, not a log or accounting ledger.
Live snapshots may span adjacent polls; closed snapshots are published after join.
Remote producer fields describe unretired registrations (including queued ones in
live readout); `retired` holds scalar lifetime totals. Local close accounts for
abandoned queued frames and flags in-flight payload assembly as unconfirmed,
without waiting for the payload. Payload unwind increments encoding loss.

`oslo make test-event-collection` checks admission, strict decoding, replacement,
capacity/age bounds, saturation, ancillary rejection, actual exec descriptor
exclusion, status reconciliation and joined worker shutdown. Retirement tests cover
capacity reuse, pending-owner exit, stale tokens, retained loss, exact deadline
abandonment, queued post-final traffic and counter saturation. Remaining launch
integration, cross-owner correlation and workload overhead acceptance are unfinished.
None of these library constructors enables collection
on the default runtime path.

## Native descriptor handoff component

`collection::bootstrap::prepare` consumes only a fresh sender. It prequeues one
strict512byte-bounded schema2/kind/owner/producer/lineage packet with exactly two
SCM_RIGHTS descriptors: event sender and original stdin. Bootstrap uses a private
nonblocking CLOEXEC Unix SEQPACKET pair; event traffic still uses datagrams.
Preparation is one attempted send before native launch, not a retry or handshake
wait. Original stdin bytes, offsets and file identity are not serialized or copied.

`receive_stdin` checks the actual parent PID/effective UID and unnamed bootstrap
endpoints. Received descriptors are CLOEXEC and owned. It rejects wrong revisions,
owners, kinds, extra/duplicate fields, malformed IDs, truncation and descriptor
counts; event sockets must be connected unnamed Unix datagrams, same-UID,
nonblocking and within the existing kernel-buffer bounds. Original input must be
readable and cannot be a packet socket. The original stdin is restored with safe
dup2_stdin before readers or children start; the event sender stays CLOEXEC.
There is no raw descriptor adoption, pre_exec, listener path or dependency change.
Validation errors close received rights without echoing payload contents.

Unnamed checks use the standard library's exact SocketAddr::is_unnamed. The cached
rustix0.38 decoder conflates unnamed addresses with empty abstract addresses;
checking abstract_name().is_none would reject genuine pairs or require an unsafe
relaxation. Inspection uses a temporary owned CLOEXEC duplicate, not a new channel.

Native exec probes exercise original-stdin restoration, event delivery and another
exec that must inherit neither bootstrap nor event descriptors. Cleanup tests use
isolated exec children: another test's concurrent fork may temporarily inherit
CLOEXEC references before exec, independently preventing an EOF leak sentinel.
All actual EOF assertions remain strict. These probes do not yet prove every
native sibling launcher, Casper jail, MCP or provider-child path.

`oslo make test-events-native` additionally exercises a selected real Casper with
Magi's bootstrap sender and collector, without a shared Rust dependency. It checks
the actual executable and restored stdin, ordinary/admitted call input, daemon key
and announcements, shell descriptor exclusion, useful results, invalid-bootstrap
refusal and reaping. Candidate casper-bootstrap-selected.log (session76044, exit0)
passes this test. Casper has explicit native run/serve activation; normal Casper
launch negotiation and remaining containment paths are unimplemented.

## Bound forwarding component

`Forwarder::activate` explicitly binds the sole Magi queue to a fresh parent-issued
Magi producer identity. Used senders, foreign owners and replacement of an existing
local receiver are refused. There is no automatic CLI/config activation.
The owned sender thread attempts one send per dequeued frame and polls every5ms;
it does not enqueue into a second userspace queue or retry failed transport sends.
Periodic status uses the existing one-attempt/second limit.

Shutdown wakes and joins the sender, closes its local receiver and reports abandoned
queued events. It never waits for an acknowledgment, drains indefinitely or detaches.
An active payload closure leaves final delivery unconfirmed. A rate-limited final
report stays unknown rather than introducing a shutdown wait; full/closed transport
and worker panic/overrun remain explicit incomplete outcomes. Useful emission stays
nonblocking with a full queue or unavailable collector. Root lifetime tokens still
belong to the native child owner and must be retained through actual reaping.

## Revision1 envelope

- `schema`: integer1; other revisions are rejected.
- `program`: `magi`; other owners define their own allowed facts independently.
- `program_version`: numeric major/minor/patch triple from the owning package.
- `producer`:128bit OS-generated lowercase hexadecimal nonce, minted during setup.
- `sequence`: nonzero monotonically allocated integer; `(producer, sequence)` is
  the event identity. Concurrent enqueue order need not match sequence order.
- `correlation`: nullable opaque run/agent/turn/request/ask/attempt/job/tool-call IDs.
  Prompt IDs derive from the producer and a checked nonwrapping ordinal with
  existing SHA256; they require no per-prompt entropy or I/O. They are not native
  request labels, paths, provider IDs or credentials. Cross-process propagation
  remains separate; unrelated correlation fields remain null.
- `fact`: typed logical-turn, permission, policy, authorization or client outcome. Turn duration is optional
  monotonic elapsed milliseconds; absent measurements remain null, never fabricated0.

There is no arbitrary text, measurement map, error detail, model name, URL, path,
request body, header or environment field. Unknown owners/kinds/fields, duplicate
fields, noncanonical IDs and invalid numeric states are rejected. Decode errors
are fixed classifications, not parser diagnostics containing input text.

## Bounds and loss

The queue holds at most256 encoded events; each encoding is at most4096bytes.
Serialization uses a fixed4096byte buffer and enqueue uses `try_send`, never a
capacity wait. Queued payload is at most1MiB; this is not a total process-memory
claim: queue metadata, one serialization buffer and payload per concurrent emitter,
and data already taken by a future consumer are separate allocations.

Full/closed/encoding/sequence-exhaustion losses have independent saturating counters.
Receiver shutdown also reports abandoned events: successful enqueues minus events
already handed to the consumer. Concurrent send/close accounting settles once
producers return; a queued result is not a delivery acknowledgment. Consumer-owned
payloads are outside the queue's delivery/loss contract.
`u64::MAX` means at least that many losses. Snapshots across counters are not atomic.
Sequences are consumed by failed attempts and never reused; maximum sequence is
reserved as terminal exhaustion. No recursive loss event, retry or spool is created.
This best-effort stream is neither a spending ledger nor a recovery journal.

Local identity exhaustion also increments the exhausted counter and leaves the
request unobserved without preventing native work. Identity and event sequence
counters are separate and never wrap.

The payload closure is owner code and must only assemble bounded typed facts;
queue nonblocking behavior does not make arbitrary caller work nonblocking.
No real-time or whole-operation latency guarantee is claimed.

## Native prompt lifecycle

Only user/workflow prompt commands entering `Preparing::queue` receive a
logical-turn ID. Declaration/grant/report maintenance, catalog refresh, child
notifications and historical replay do not become new prompt turns. The same
move-only trace passes through the immediate or asynchronous preparation path,
through submission, and into native session admission. Disabled or exhausted
identities are not regenerated at a later stage.

Malformed workflow invocations, invalid images and preparation-budget failures
emit `refused` without admission. Asynchronous preparation emits `preparing` after
its budgets are reserved, before channel handoff; this is not session admission,
active execution or proof of a successful handoff. A closed handoff emits a
matching `refused`. Native ticket cancellation and generation invalidation emit
`cancelled`; the unchanged45second enqueue deadline emits `expired`. Ordinary
preparation errors remain `refused`. Destruction of a preparation owner without
a selected terminal result is `unknown`, not fabricated cancellation or success.
The256-job/64MiB native limits and ticket-held permit lifetimes are unchanged.

Successful admission emits `admitted`; busy ordinary requests also emit `queued`.
The same ID follows the bounded native queue into `started` when it acquires active
ownership. A busy tracked request is `refused`, not queued. Started denotes native
activation, not proof of a provider request. Stale completion cannot finish the
current owner's trace, and activation does not repeat admission.

After handling and the configured history flush, the latest assistant outcome
since activation determines completion. Handler/flush errors and a final assistant
error are `failed`; an aborted response is `cancelled`; EndTurn/Length are
`completed`. A cancellation flag does not rewrite an already completed response.
Otherwise cancellation is reported if requested, and missing terminal evidence
is `unknown`. An earlier recovered error does not override a later successful
answer. Completed is not a durability claim: operation without a history store
can complete without persisting a transcript.

Terminal duration is monotonic time since this prompt entered preparation,
including preparation and admission queue wait; transition durations are null.
Dropping never-started session-queued work reports
cancelled; losing active ownership without a proven outcome reports unknown.
Normal completion emits at most once. Forced process termination need not emit
anything. When disabled, these traces obtain neither clocks nor IDs and do not
scan the transcript at completion.

## Permission requests

Each local `Asker` permission request or parent review receives an opaque local
`request_id`. Other correlation remains null; native approval IDs, controller
keys, tool names, paths, action bodies, advice and grant scopes never enter these
events. General choice questions are not permission requests.

- `offered`: the local question was registered and its publication callback
  returned. This is not proof of screen delivery or human visibility.
- `forwarded`: the native parent-review request was written. It is not a second
  local offer or proof that the parent displayed a question.
- `allowed`/`denied`: the request received that exact native decision. A parent
  reply may come from rules or fail-closed behavior, not necessarily a person.
- `cancelled`: the native stop/withdrawal predicate ended the request.
- `expired`: local patience or a native socket timeout expired.
- `unavailable`: no attached person, native question setup failure, answer-channel
  closure, unavailable parent, peer refusal or invalid/missing transport reply.
  An unexplained channel closure is not labelled user denial or cancellation.
- `unknown`: observation ownership unwound without a known terminal result.

The trace emits at most one terminal outcome. Pending-question replay and stale
answers create no new offer or decision. Native fail-closed decisions, grant scope,
timeouts, polling and forwarded reader-thread joining remain unchanged. Allowed
does not mean a grant was persisted or a tool executed. Parent and child traces
are distinct owner-local requests; they cannot be summed as unique human decisions
without a separately specified correlation policy. No permission duration is
claimed, and disabled traces perform no telemetry ID allocation or entropy I/O.

## Client transport lifecycle

The native accept loop emits `connected` after same-user peer validation and
connection-seat admission, before a handler is spawned. Its guard emits
`disconnected` when handler ownership ends, including a task dropped before its
first poll. These facts share one opaque local `request_id`; no native socket
path, peer UID, controller key or session label is emitted. Unrelated correlation
stays null, and a reconnect receives a fresh ID.

Connected does not mean attached, authenticated as controller or authorized to
submit commands. Inspect, history and review handlers are connections too. Native
watchers remain valid clients; rejecting their commands is not a new connection
refusal. `refused` records failed peer validation or exhausted connection capacity
before admission, without a connected/disconnected pair. Disconnected describes
owner-handler lifetime, not proof that every asynchronous reader or OS descriptor
has already been reaped. Forced process termination need not emit a final event.

The guard holds only an optional ID, performs no clock/entropy I/O and preserves
native peer checks,64-seat capacity, timeouts, task cleanup and command authority.
Historical replay does not create additional connection facts. A connection that
began with events disabled remains unobserved if activation happens later.

## Completed authorization

The worker installs an enum-only observer on its native `Real::allow` gate. Every
completed gate call emits at most one `authorization` fact: `ungated`,
`standing_grant`, `approver_allowed` or `approver_denied`. A rule override still
takes precedence over a standing grant. Approver outcomes describe the native
result, not a human decision: automatic edits, locked mode, forwarded requests
and interactive answers can all pass through that authority.

Each observation has a fresh opaque local `request_id` with unrelated correlation
null. These are not permission-request or tool-call IDs; adding their counts to
permission events does not measure distinct user decisions. No tool/action name,
path, scope, refusal reason or caller-controlled label reaches the callback.
Events are sampled at completion, without retaining an in-flight trace or clock.
An approver unwind does not fabricate a completed authorization.

The native result, grant ledger and notices are settled before observation. The
trusted owner callback cannot change the return value, and ordinary callback
unwinding is caught. Disabled observation obtains no ID; a full or closed queue
only adds explicit loss. No event can grant permission. Authorization does not
prove an operation ran, succeeded, or persisted a grant; an allowed write that
fails afterward still has an allowed authorization outcome. Third-party `Ops`
implementations and arbitrary custom approver internals are not instrumented here.

## Native policy provenance

`Judged` emits one completed `policy` fact at its actual decision branch. Fixed
rule-denied and locked-denied states cannot encode allowed outcomes. Local-allowed
states identify Auto or Edits; model judgments carry allowed/denied outcomes.
Delegated decisions identify an ask rule, ambiguous program rule, Ask/Edits mode,
missing/uncertain judgment or refusal-limit fallback, with the returned outcome.
Missing judgment means the native judge returned none, not proof of a network error.
Delegation does not mean a human answered: the delegated approver can itself apply
policy, forward, refuse or interact. No rule text, advice, confidence, model label,
action, scope or refusal detail is emitted.

The native decision, notices and counters settle before observation; exact grant
scope/lifetime and refusal behavior are unchanged. Judge, delegate or notice
unwinding does not fabricate a completed policy decision. A standing grant that
bypasses `Judged` emits no policy fact. No started event, retained trace or new
deadline is introduced. Disabled/full/closed observation cannot change authority.

Policy, permission and authorization observations describe different stages, not
separate executions, human decisions or provider charges. Each policy fact has its
own local opaque request identity; linkage to other stages remains separate. A
completed policy result does not prove ledger persistence or actual tool success.

## Verification

Native memory requests negotiate revision1 from successful verbs replies. The
bounded capability registry holds at most32paths of4096bytes, paired with the
actual peer PID; every connection checks that peer and euid. Context stays outside
arguments and disappears when the host scope is absent. Lua-provided outer context
is removed and replaced only by negotiated host context; nested arguments remain
data. `oslo make test-events-memory` exercises actual JSON/CBOR clients, queue
admission and existing connection ownership. `oslo make test-events-native` also
checks selected Balthasar layout events across turns, missing/invalid contexts,
foreign peers and Lua forgery. These are not full-family or overhead acceptance.

`oslo make test-events` runs exact golden-schema, hostile-input, disabled-path,
encoder/decoder limits, saturation, closure, sequence exhaustion, concurrent
producer and typed-fact tests. A barrier-controlled producer progresses with a
full queue while the consumer remains stalled. Isolated native admission tests
check ownership, queue identity, stale completion and dropped work. Thirteen disposable
socket/worker scenarios cover success, provider failure, missing worker, configured
store failure after a successful answer, cancellation/reaping, queueing/busy refusal,
history replay and disabled/full/closed event queues. Useful work still finishes
when observation cannot enqueue; content canaries stay out of encoded events.
The additional socket cases verify malformed invocation, absent history and invalid
image refusals before any turn starts, then successful work on the same connection.

Twenty isolated native preparation scenarios check immediate transfer, validation
refusals, ticket drop, cancellation/invalidation, enqueue-clock expiry, missing
history, asynchronous admission, session queue destruction, count/byte limits,
closed handoff, owner abort, maintenance exclusion and disabled/late/full/closed
observation. Expiry uses the native45second deadline with Tokio's paused test clock,
not a shortened production limit. The selected-native workflow submission suite
also verifies one identity through Casper acquisition and Balthasar history:
source changes do not change replay or produce new activity. Its observation case
deliberately has no model worker and ends `failed`, while the workflow snapshot is
retained; preparation/admission is not successful model execution.

Sixteen additional disposable native Asker/Unix-socket scenarios check exact grant
scope, explicit answers versus unavailable delivery, replay/stale answers, stop,
forwarded cancellation and reader joining, missing/malformed/disconnected parents,
publication panic and disabled/full/closed queues. An isolated native short-patience
test checks expiry and one terminal emission; socket error classification is tested
separately without waiting out the production305second read timeout.

Fifteen native client scenarios check inspect/attach/watchers, reconnect/replay,
EOF, invalid/malformed handshakes, the actual30second attach deadline, valid and
denied history reads, unauthorized review,64-seat saturation and useful replies
under disabled/full/closed event queues. A separate guard test covers unpolled
task drop and refusal identity. These do not exercise a foreign-UID peer in a
cross-user integration fixture. Turn tests explicitly check their initial client
connection separately from logical turns and include its enqueue loss.

Seven native gate tests check absent observers, ungated/confined constructors,
standing and once grants, rule overrides, exact refusals/notices and callback or
approver unwinding. Twenty-two isolated real-worker/Lua scenarios verify actual file
writes and refusals, native Auto/Edits/Locked policies, native Asker answers,
distinct permission and authorization IDs, grant reuse, allowed-but-failed writes,
and useful work under disabled/full/closed queues. Automatic policy emits no
permission request; each actual request has its own offered/answer pair.
Two cases compose actual Judged/Asker/Worker layers and require separate permission,
delegated-policy and authorization facts while checking real writes/refusals.
Twenty-one isolated native policy scenarios cover every provenance branch, exact
returned scopes, judge/delegate call counts, refusal-limit fallback and counter
reset, unwinding and disabled/full/closed queues. Independent schema tests reject
impossible fixed-state combinations and arbitrary content-bearing fields.

`oslo make test-events-host` additionally runs all host and tools unit tests and existing turn,
lifecycle, arrivals, roundtrip and report-delivery integration suites. These checks
do not establish collector cancellation, end-to-end billing reconciliation,
selected-family acceptance or measured production overhead.
