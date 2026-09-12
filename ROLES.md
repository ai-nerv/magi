# The roles, revision 1

`FAMILY.md` says how these programs talk. This says what they are *for*.

The two are separate on purpose. A program can implement the family contract perfectly and still be
useless as a memory layer, because the family contract is about the reply shape and the doors —
not about which verbs mean anything. This document names each **role**, and the verbs a program
must answer to fill it.

**A role is filled by one program, named in configuration.** magi does not know that its memory is
called `balthasar`; it knows that the `memory` role is filled by whatever `magi.memory` names, and
that whatever fills it answers the verbs below. Change the name, and the program changes.

This document is the canonical text. It is copied into each repository beside the code that
implements it, and each `scripts/gate-<role>.sh` is the executable version: point it at a binary
and it reports which of the role that binary implements.

---

## What a role contract is

| | |
|---|---|
| **core** | Refuse one of these and the program cannot fill the role. The gate fails. |
| **extension** | Refuse one and magi carries on with less. The gate says so and passes. |

**The split is derived from what magi needs, not from what any implementation offers.** Every verb
below was found by reading magi's call sites — `magi-host/src/scribe.rs` for the harness half,
`config/tools.lua` for the model-facing half — and asking of each: *if this were refused, could the
session still run?* A verb an implementation happens to serve, that magi never calls, is not in
this document and is that implementation's own business.

**An extension is refused, not omitted.** A program that does not implement `plan` answers `plan`
with `{"ok":false,…,"fault":"refused"}` like any other refusal. It stays in `verbs`, because
"advertised equals dispatched" is a family rule and a verb answered by a refusal *is* answered.
Silence is the one thing that is not allowed.

---

## The `memory` role

A memory layer records what a session said, gives it back, and — if it can — helps decide what to
send next.

### Core

| verb | shape | what magi does with it |
|---|---|---|
| `observe` | `(session, turn) -> ok` | Streams every turn as it settles. Without this nothing is recorded and the session has no history at all. |
| `replay` | `(session) -> [turn]` | Everything a run said, in order. This is what `--resume` reads back. |
| `sessions` | `-> [session]` | The runs this project has had. `--resume` with no id asks this **first**, to find which run was newest, and only then replays it. |

Plus the family floor from `FAMILY.md` — `verbs` and `client` — which every program owes whatever
its role.

Three verbs, because three is what "a store" means: it takes the conversation, it says which
conversations it has, and it gives one back. Everything else makes it a *good* memory layer rather
than a memory layer.

**`sessions` is core because it was measured to be, not because it looked important.** It was an
extension in this document's first draft, on the reading that losing it only cost the ability to
list earlier runs. A shim written against that draft — `examples/remembrance/`, which answers the
core and refuses everything else — was pointed at magi with `sessions` refused, and `--resume`
returned an empty conversation **at exit 0, saying nothing**. `host.rs`'s `resumable()` calls
`sessions()` to find the newest id before it can call `replay`, so a memory layer without it is
silently unresumable. That is the whole reason a role contract is worth writing: the boundary was
drawn wrong, and only running a program against it found out.

### Extensions

| verb | shape | what magi loses without it |
|---|---|---|
| `amend` | `(session, turn) -> ok` | A turn revised after it settled keeps its first text. magi writes `observe` at a cursor that already has a row, and an implementation may treat that as an amend. |
| `recall` | `(query, opts) -> [memory]` | The model's `recall` tool is not declared. |
| `remember` | `(text, opts) -> landing` | The model's `remember` tool is not declared. |
| `forget` | `(id, opts) -> ok` | The model's `forget` tool is not declared. |
| `why` | `(id) -> { confidence, witnesses }` | The model's `why` tool is not declared. |
| `scroll` | `(session, opts) -> { turns, next }` | The model's `history` tool is not declared, and an elided tool result cannot be read back. |
| `plan` | `(session, window) -> { keep, mask, drop, summarise, why }` | No compaction. An over-budget context goes to the provider and is refused there. This is the most expensive extension to lack. |
| `used` | `(injection, opts) -> { action }` | The outcome loop records nothing. |
| `outcome` | `(action, opts) -> { outcome, kind }` | The outcome loop records nothing. |
| `model` | `(session, opts) -> { model, context }` | The store does not know which model produced a run. |
| `resume` | `(session) -> { next, turns }` | magi cannot cross-check that the store holds what this session thinks it does. |

**The four model-facing verbs — `recall`, `remember`, `forget`, `why` — are declared
independently.** magi declares a tool for each verb the role answers and omits the rest, so a
memory layer that can search but not explain gets three tools and no `why`.

---

## The `tools` role

A tool layer offers named tools, runs one on request, and may hold rows on the harness's screen.

### Core

| verb | what magi does with it |
|---|---|
| `tools` | Every tool it offers, with schemas. An empty answer is legal and means "no tools". |
| `run` | Run one; the call arrives as JSON on stdin. |

Plus the family floor.

### Extensions

| verb | what magi loses without it |
|---|---|
| `surface` | No tool of this program's can hold rows on the screen; the drawing tools are not declared. |
| `acknowledge` | Installed packages cannot be cleared for running, so a fetched package's declarations never run. |

**A tools program may keep a socket, but the trust is never the caller's.** casper binds one with
`serve` and answers `tools` and `run` on it, kept open across a session so a call need not spawn a
process each time. What makes that safe is that the jail is set on `serve`'s spawn — in the
program's environment, by the coordinator — never by the call: a socket peer runs inside the same
walls a spawned call would, and a peer of another user is turned away. See `FAMILY.md`'s Doors
table. The command line stays the spawn-per-call door and is always available.

**Its settings arrive in `MAGI_TOOLS_CONFIGURE`**, as a JSON object, on every spawn — `tools` and
`run` alike, since there is no process alive between calls to send them to once. magi reads them
from the configuration table named after the program itself, so `magi.tools = "workbench"` is told
what `magi.workbench = { … }` says. Empty means nothing was said. `CASPER_CONFIGURE` carries the
same value for the program that filled this role before it had a name; read either.

---

## The `model` role

The program that owns which models exist, what each costs, and how to speak to one.

This role is **already swappable** — `magi.melchior` has named its program since before this
document — and its contract is the one least exercised by a second implementation. It is written
here for completeness and should be treated as provisional until something other than melchior
fills it.

### Core

| verb | what magi does with it |
|---|---|
| `models` | The catalogue: which endpoints exist and what they offer. |
| `ask` | One turn against a model, as a stream. |

Plus the family floor.

Everything else melchior answers — the session surface, the coordination vocabulary on its `tool`
door — belongs to the `coordination` role, which is not written down yet because nothing has ever
tried to fill it separately. See the open question at the end.

---

## When a role is unfilled

**`memory` unfilled is a refusal to start.** magi says so today: *"balthasar is the store — there
is no local journal to fall back to."* A session that cannot record is refused rather than run,
because a JSONL fallback made two stores, one of them going stale silently, and a session resumed
from it resumes into something that half happened.

**`tools` unfilled is an ordinary session with no tools.** It has always been legal.

**`model` unfilled is a refusal to start**, for the obvious reason.

---

## What moves these numbers

The same rule as `FAMILY.md`'s two versions. This document is **revision 1**. It goes up when a
program that filled a role stops filling it: a core verb added, a verb's shape changed, an
extension promoted to core. Adding an *extension* moves nothing — a program that does not answer it
refuses it, which is what it would have done before the verb existed.

---

## Joining as a role

Implement the core, answer the family contract, and pass `scripts/gate-<role>.sh`. Refuse the
extensions you do not implement, by name, in the reply shape. Nothing else is required — there is
no registration and no shared library, for the reason `FAMILY.md` gives.

---

## Open questions, deliberately not answered here

- **`coordination` is not a role yet.** melchior's `tool` door — `crew`, `claim`, `send`, `assign`,
  `disband` and the rest — is a substantial surface that nothing has tried to reimplement. Writing
  it down before a second implementation exists would describe melchior rather than the need.
- **magi is not a role.** It coordinates and is not coordinated. A fifth program wanting to *be*
  the coordinator gets no help from this document.
