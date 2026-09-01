# axon — the scrollback, moved out

> The other half of `aeon`'s §3.7. Written from aeon's side, for whoever implements this one.
> aeon's half is built: the store, the verbs, and the tests are in `../aeon` at `5a4dd7b`+.

---

## What is being decided

axon stops keeping a journal. `aeon` becomes the only copy of what was said, and axon **cannot
start a session without it**.

This reverses the recommendation in aeon's `PLAN.md` §3.7, which argued for keeping the journal
as a write-ahead log and treating aeon's transcript as a queryable mirror. That argument is
still in that file and still worth reading before committing to this. It was overruled
deliberately, and what follows is built for the decision that was actually made rather than the
one that was recommended.

**What changes about the failure mode.** Today, killing aeon costs memory and the turn keeps
running. After this, killing aeon costs the session. Everything below that looks like
over-engineering — the durability pragma, the acknowledge-before-proceed rule, the refusal to
start — is there because that failure mode is now the one that matters.

---

## 1. Where it lives

```
$XDG_DATA_HOME/aeon/
  thing-1a2b3c.db              the memory store — small, rewritten constantly
  thing-1a2b3c-transcript.db   the scrollback  — large, append-mostly    ← this
```

Two files, on purpose. A transcript is roughly three orders of magnitude larger than the
memories distilled from it; sharing a file would make every recall walk past it and leave a
retention policy nowhere to bite.

The scrollback is opened with `PRAGMA synchronous = FULL`, not the WAL default of `NORMAL`.
NORMAL can lose the last transactions to a power cut while keeping the database consistent —
the right trade for a cache and the wrong one for the only copy. **This is stronger than what
`axon-journal` does today**, which flushes but does not `fsync` (`Journal::append` calls
`writer.flush()`; see its own comment about why per-token fsync would be ruinous).

---

## 2. The four verbs

All over the existing family socket — same framing, same reply shape, same stub. Nothing new to
transport.

| verb | arguments | answers |
|---|---|---|
| `observe` | `(session, turn)` | nothing. The turn is durable when this returns. |
| `amend` | `(session, turn)` | nothing. Revises the turn already at that cursor. |
| `replay` | `(session)` | `[turn]`, in cursor order, as they finally stood |
| `resume` | `(session)` | `{ next, turns }` — where to carry on from |

`observe` and `amend` differ only in intent; both write to `(session, cursor)` and the second
write wins. Two verbs rather than one because a harness that means *revise* and a harness that
means *append* should not be told apart by whether a row happened to exist.

---

## 3. The turn shape

What axon sends. Everything but `cursor` is optional, and `raw` is the one that matters.

```jsonc
{
  "cursor": 41,              // axon's own numbering. aeon never renumbers.
  "at":     1756600000,      // unix seconds. defaults to aeon's clock.
  "role":   "assistant",     // user | assistant | tool
  "kind":   "prose",         // prose | thinking | tool_result | summary
  "text":   "…",             // for quoting and for search. NOT the record.
  "tool":   "shell",         // when it is one

  "raw":    { … }            // ← THE RECORD. aeon stores it and never parses it.
}
```

**`raw` is the contract.** Send the serialised `axon_proto::Record` — the same line the journal
would have written. aeon keeps it as an opaque string, hands it back byte for byte on `replay`,
and has no idea what an `Entry` is. That is what keeps aeon's commitment 1 intact while it holds
axon's only copy: a second harness with entirely different records needs no change in aeon.

Either a JSON object or a pre-serialised string is accepted; both come back identically.

`text` exists so aeon can quote a turn in `aeon why` and search one. It is a projection, not the
record — losing it costs a nicer diagnostic, not a session.

### Mapping `axon_proto::Entry`

`Record` is `{"record":"entry","cursor":N,"entry":{…}}`, and `Entry` is internally tagged on
`type`, snake_case. Six variants, and only five of them ever reach here:

| `Entry` | `role` | `kind` | `text` |
|---|---|---|---|
| `User { id, text }` | `user` | `prose` | `text` |
| `Assistant { text, thinking, stop_reason, … }` | `assistant` | `prose` | `text` |
| `Tool { name, args, result }` | `tool` | `tool_result` | `result.output` |
| `Branch { keeps }` | `assistant` | `summary` | `""` |
| `Compaction { summary, replaces }` | `assistant` | `summary` | `summary` |
| `Notice { text }` | — | — | **never sent.** Its own doc says it is never journalled: it is one UI's commentary, not the conversation. |

Everything the table drops — `id`, `signatures`, `usage`, `thought_signature`, `stop_reason`,
`keeps`, `replaces` — rides in `raw` and comes back intact. The table is only what aeon needs to
*show* a turn.

### The amend case, spelled out

`Entry::Tool` is written with `result: None` while the call runs and amended when it lands.
That is the whole reason `amend` exists:

```
  observe(session, { cursor: 41, role: "tool", text: "",   raw: { …result: null } })
  amend  (session, { cursor: 41, role: "tool", text: "ok", raw: { …result: {…}  } })
```

`replay` answers the second form. `revisions` on the returned turn counts how many times the
cursor was rewritten — 0 for a turn written once, 1 for a tool call that got its result.

---

## 4. The rules that stop this losing data

Four, and they are the whole of it.

**Acknowledge before proceeding.** `observe` returning is the only signal that a turn is safe.
Do not render it as settled, do not advance the cursor, do not start the next turn until it
does. Fire-and-forget was correct when axon had its own journal and is data loss now.

**Refuse to start rather than start without it.** If `aeon` cannot be reached at session open,
axon must say so and stop. A session that begins and cannot persist is worse than one that never
began: the person will have said things to it.

**Allocate cursors from `resume`.** On restart, ask `resume` first and continue from `next`.
Guessing overwrites a turn that nothing else holds a copy of.

**One writer per session.** `(session, cursor)` is the primary key and last write wins. Two
daemons on one session will silently interleave. `SO_PEERCRED` identifies the caller but does
not exclude a second one — if two axons can hold one session, that is axon's lock to take.

---

## 5. Restoring

```lua
local held = memory.resume(session)        -- { next = 412, turns = 411 }
if not held then error("aeon is unreachable; this session cannot be restored") end

for _, turn in ipairs(memory.replay(session)) do
  local record = json.decode(turn.raw)     -- exactly the line the journal held
  transcript:push(record.entry)
end
cursor = held.next
```

`replay` answers in cursor order, final form. A turn aeon holds no `raw` for is **skipped, not
invented** — a replay that made something up would be worse than a short one, so a `raw` missing
here is a bug on axon's side to find, not a hole for aeon to paper over.

The existing view logic is unaffected. `context.rs` rebuilds the provider conversation from
entries and does not care where they came from; `Branch` and `Compaction` still resolve as views
over what `replay` returns.

---

## 6. What axon can delete, and what it must not

**Can go:** the JSONL files, `paths::latest_for`, `Journal::open`'s recovery path, the
torn-tail truncation.

**Must stay:** `Record` and `Entry` — they are still the serialisation, they just land somewhere
else. `Cursor` allocation stays axon's. `context.rs` is untouched.

**Worth keeping anyway:** `axon-journal`'s conformance tests. They describe what a transcript
has to survive, and they are now describing aeon's job.

---

## 7. What is already built and testable

```sh
aeon serve                    # prints the scrollback path it opened
aeon replay                   # the runs it holds
aeon replay <session>         # the turns, for a person
aeon replay <session> --raw   # the records, one per line — what a restore reads
aeon replay <session> --resume
aeon why <handle>             # now quotes the turn each witness saw
```

Round-tripped in `crates/aeon-host/tests/scrollback.rs`: a record comes back byte for byte, the
same turn twice is two turns, a tool call revises in place, `resume` reports the next cursor,
`why` quotes, and a host with no scrollback refuses `replay`/`resume`/`amend` rather than
answering emptily.

---

## 8. Two things to settle before writing any axon code

**Retention.** Nothing prunes the scrollback yet. A year of sessions is a large file and there is
no policy for it — by age, by session count, or never. Memory decay does not touch it, by
design.

**Concurrency.** Rule four above is unenforced. If two axon daemons can ever hold one session,
decide now whether the lock is axon's or aeon's, because retrofitting it means a schema change.
