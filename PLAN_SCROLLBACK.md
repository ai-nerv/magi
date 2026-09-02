# magi — the scrollback, moved out

> The other half of `balthasar`'s §3.7. Written from balthasar's side, for whoever implements this one.
> balthasar's half is built: the store, the verbs, and the tests are in `../aeon` at `5a4dd7b`+.

---

## What is being decided

magi stops keeping a journal. `balthasar` becomes the only copy of what was said, and magi **cannot
start a session without it**.

This reverses the recommendation in balthasar's `PLAN.md` §3.7, which argued for keeping the journal
as a write-ahead log and treating balthasar's transcript as a queryable mirror. That argument is
still in that file and still worth reading before committing to this. It was overruled
deliberately, and what follows is built for the decision that was actually made rather than the
one that was recommended.

**What changes about the failure mode.** Today, killing balthasar costs memory and the turn keeps
running. After this, killing balthasar costs the session. Everything below that looks like
over-engineering — the durability pragma, the acknowledge-before-proceed rule, the refusal to
start — is there because that failure mode is now the one that matters.

---

## 1. Where it lives

```
$XDG_DATA_HOME/balthasar/
  thing-1a2b3c.db              the memory store — small, rewritten constantly
  thing-1a2b3c-transcript.db   the scrollback  — large, append-mostly    ← this
```

Two files, on purpose. A transcript is roughly three orders of magnitude larger than the
memories distilled from it; sharing a file would make every recall walk past it and leave a
retention policy nowhere to bite.

The scrollback is opened with `PRAGMA synchronous = FULL`, not the WAL default of `NORMAL`.
NORMAL can lose the last transactions to a power cut while keeping the database consistent —
the right trade for a cache and the wrong one for the only copy. **This is stronger than what
`magi-journal` does today**, which flushes but does not `fsync` (`Journal::append` calls
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

What magi sends. Everything but `cursor` is optional, and `raw` is the one that matters.

```jsonc
{
  "entry":  "e-7a1f",        // which message this block is part of. Blocks share it.
  "cursor": 41,              // magi's own numbering. balthasar never renumbers.
  "at":     1756600000,      // unix seconds. defaults to balthasar's clock.
  "role":   "assistant",     // user | assistant | tool
  "kind":   "prose",         // prose | thinking | tool_call | tool_result | summary
  "text":   "…",             // for quoting and for search. NOT the record.
  "tool":   "shell",         // when it is one

  "raw":    { … }            // ← THE RECORD. balthasar stores it and never parses it.
}
```

**`raw` is the contract.** Send the serialised `magi_proto::Record` — the same line the journal
would have written. balthasar keeps it as an opaque string, hands it back byte for byte on `replay`,
and has no idea what an `Entry` is. That is what keeps balthasar's commitment 1 intact while it holds
magi's only copy: a second harness with entirely different records needs no change in balthasar.

Either a JSON object or a pre-serialised string is accepted; both come back identically.

`text` exists so balthasar can quote a turn in `balthasar why` and search one. It is a projection, not the
record — losing it costs a nicer diagnostic, not a session.

### Mapping `magi_proto::Entry`

`Record` is `{"record":"entry","cursor":N,"entry":{…}}`, and `Entry` is internally tagged on
`type`, snake_case. Six variants, and only five of them ever reach here:

**One block, one turn.** An `Assistant` entry carries prose, possibly a thought, and possibly
several tool calls. Each becomes its own turn at its own cursor, and they share an `entry` — the
entry's own `id`. That is what lets a span address one tool call rather than only the message
around it, and it is what stops a bounded read handing back an assistant turn without the call
it made: balthasar drops a leading part-message rather than showing a fragment of one.

| `Entry` | blocks | `role` | `kind` | `text` |
|---|---|---|---|---|
| `User { id, text }` | one | `user` | `prose` | `text` |
| `Assistant { thinking, … }` | one, if present | `assistant` | `thinking` | `thinking` |
| `Assistant { text, … }` | one, if non-empty | `assistant` | `prose` | `text` |
| `Assistant { calls }` | **one per call** | `assistant` | `tool_call` | the command or a rendering of the arguments |
| `Tool { name, args, result }` | one | `tool` | `tool_result` | `result.output` |
| `Branch { keeps }` | one | `assistant` | `summary` | `""` |
| `Compaction { summary, replaces }` | one | `assistant` | `summary` | `summary` |
| `Notice { text }` | — | — | — | **never sent.** Its own doc says it is never journalled: it is one UI's commentary, not the conversation. |

**`entry` is the entry's `id`**, on every block of it. A turn that is its own message may leave
it out; a plain `User` turn does.

**`raw` goes on the first block only.** Restoring reassembles *messages*, not blocks, so the
record belongs to the message — written once, and `replay --raw` emits it once. The other blocks
exist to be read and addressed, never to be restored from. A turn balthasar was given no record for
is skipped on replay rather than invented, which is what makes this work without a second rule.

Everything the table drops — `id`, `signatures`, `usage`, `thought_signature`, `stop_reason`,
`keeps`, `replaces` — rides in `raw` and comes back intact. The table is only what balthasar needs to
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
does. Fire-and-forget was correct when magi had its own journal and is data loss now.

**Refuse to start rather than start without it.** If `balthasar` cannot be reached at session open,
magi must say so and stop. A session that begins and cannot persist is worse than one that never
began: the person will have said things to it.

**Allocate cursors from `resume`.** On restart, ask `resume` first and continue from `next`.
Guessing overwrites a turn that nothing else holds a copy of.

**One writer per session.** `(session, cursor)` is the primary key and last write wins. Two
daemons on one session will silently interleave. `SO_PEERCRED` identifies the caller but does
not exclude a second one — if two magi sessions can hold one session, that is magi's lock to take.

---

## 5. Restoring

```lua
local held = memory.resume(session)        -- { next = 412, turns = 411 }
if not held then error("balthasar is unreachable; this session cannot be restored") end

for _, turn in ipairs(memory.replay(session)) do
  local record = json.decode(turn.raw)     -- exactly the line the journal held
  transcript:push(record.entry)
end
cursor = held.next
```

`replay` answers in cursor order, final form. A turn balthasar holds no `raw` for is **skipped, not
invented** — a replay that made something up would be worse than a short one, so a `raw` missing
here is a bug on magi's side to find, not a hole for balthasar to paper over.

The existing view logic is unaffected. `context.rs` rebuilds the provider conversation from
entries and does not care where they came from; `Branch` and `Compaction` still resolve as views
over what `replay` returns.

---

## 6. What magi can delete, and what it must not

**Can go:** the JSONL files, `paths::latest_for`, `Journal::open`'s recovery path, the
torn-tail truncation.

**Must stay:** `Record` and `Entry` — they are still the serialisation, they just land somewhere
else. `Cursor` allocation stays magi's. `context.rs` is untouched.

**Worth keeping anyway:** `magi-journal`'s conformance tests. They describe what a transcript
has to survive, and they are now describing balthasar's job.

---

## 7. What is already built and testable

```sh
balthasar serve                    # prints the scrollback path it opened
balthasar replay                   # the runs it holds
balthasar replay <session>         # the turns, for a person
balthasar replay <session> --raw   # the records, one per line — what a restore reads
balthasar replay <session> --resume
balthasar why <handle>             # now quotes the turn each witness saw
```

Round-tripped in `crates/balthasar-host/tests/scrollback.rs`: a record comes back byte for byte, the
same turn twice is two turns, a tool call revises in place, `resume` reports the next cursor,
`why` quotes, and a host with no scrollback refuses `replay`/`resume`/`amend` rather than
answering emptily.

---

## 8. Two things to settle before writing any magi code

**Retention.** Nothing prunes the scrollback yet. A year of sessions is a large file and there is
no policy for it — by age, by session count, or never. Memory decay does not touch it, by
design.

**Concurrency.** Rule four above is unenforced. If two magi daemons can ever hold one session,
decide now whether the lock is magi's or balthasar's, because retrofitting it means a schema change.
