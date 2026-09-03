# What balthasar has to hold for magi to draw the scrollback again

> The companion to `PLAN_SCROLLBACK.md`. That one is written from balthasar's side and says how
> the store works. This one is written from magi's side and says what magi will hand over, what
> it needs back, and what breaks if any of it is lost.
>
> Written against magi at `f507e3d`. Every claim below cites the tree.

## Why this exists

Once balthasar holds the only copy, `replay` is not a convenience — it *is* the transcript. Magi
draws seven kinds of block, and each carries fields that nothing else can reconstruct: a
provider signature that must come back byte for byte, a count that indexes the transcript it
lives in, a per-turn cost that a resumed session shows instead of starting from zero. Anything
balthasar normalises, reorders, or helpfully cleans up is a session that comes back subtly wrong,
and subtly wrong is worse than absent because nobody checks.

## 1. The vocabulary

`magi_proto::Entry` (`crates/magi-proto/src/lib.rs:166`) — seven variants. Five are written to
the journal; two are made by a UI and never stored.

| kind | fields | journalled | notes for reconstruction |
|---|---|---|---|
| `user` | `id`, `text`, `aside` | yes | `aside` is context the model sees and **nobody is shown** (`:173-183`). Stored, never drawn. |
| `assistant` | `id`, `text`, `thinking`, `stop_reason`, `error`, `signatures`, `usage` | yes | five fields beyond the prose, all load-bearing. See §2. |
| `tool` | `id`, `name`, `args`, `result`, `thought_signature` | yes | `result` is `None` while running; `ToolResult { output, is_error }` (`:154`). |
| `from` | `who`, `kin`, `sort`, `text` | yes | another session speaking. `kin` is carried, not looked up, because a session that has since forked would redraw history with a relation that did not hold then (`:250-253`). |
| `branch` | `id`, `keeps` | yes | `keeps` is a **count of entries from the start**, not a cursor. |
| `compaction` | `id`, `summary`, `replaces` | yes | `replaces` is likewise a count. |
| `notice` | `text` | **no** | magi talking, not the model. Never journalled (`crates/magi-host/src/session.rs:437`). Not balthasar's to hold, and a replayed session correctly has none. |

`PLAN_SCROLLBACK.md` §3 offers `kind` as `prose | thinking | tool_call | tool_result | summary`.
That covers assistant prose, thinking, the two halves of a tool call, and compaction. It has no
value for **`user`**, **`from`** or **`branch`**. The projection needs those three added, or
balthasar will label a message from a sibling session as ordinary prose and `balthasar why` will
quote it as though the person said it.

## 2. What only survives if it is stored exactly

Four fields where a helpful transformation is data loss.

**`signatures` and `thought_signature`** — opaque provider state that the next request must carry
verbatim or the provider returns 400 (`crates/magi-proto/src/lib.rs:293-299`). Anthropic's
extended thinking with tools makes this mandatory, not nice. Google issues one per *call* rather
than per message, which is why `thought_signature` rides on the tool entry instead of the message
that asked for it (`:217-220`). No re-encoding, no whitespace normalisation, no unicode
normalisation. Bytes.

**`usage`** — journalled rather than counted live, so a resumed session shows the totals it
actually accrued instead of starting again from zero (`:202-205`). Recomputing it on replay is
not possible; the token counts came from the provider.

**`keeps` and `replaces`** — counts that index the transcript they are stored in. This is the
requirement that makes cursor stability non-negotiable: if balthasar renumbers, drops, or
reorders anything before a `branch`, that branch now cuts the conversation in the wrong place.
`PLAN_SCROLLBACK.md` already promises "balthasar never renumbers"; this is why it matters.

**`stop_reason` and `error`** — a failed turn renders differently from a finished one, and
`error` is populated only when `stop_reason` is `Error`. A turn that comes back with both dropped
looks like it succeeded.

## 3. The hard requirements

Reconstruction reads `raw` — the serialised `magi_proto::Record`, which balthasar stores opaquely
and never parses. So the list is short, and deliberately so:

1. **`raw` returns byte-identical.** Not equivalent JSON. The same bytes.
2. **Order is by cursor, ascending**, never by arrival or by `at`. Magi assigns cursors
   (`crates/magi-journal/src/record.rs:29-34`); balthasar preserves them.
3. **Amendment replaces at a cursor and the second write wins.** Magi amends settled entries —
   `Journal::amend_at` (`crates/magi-journal/src/lib.rs:231`) replaces the entry at a cursor
   wherever it sits, and a streaming assistant message is amended repeatedly as it grows.
   `replay` must return only the final state.
4. **Gaps are legal and are not an error.** A cursor sequence with holes must replay as what is
   there, not as a failure.
5. **`observe` is durable when it returns.** Already specified as `PRAGMA synchronous = FULL`,
   and stronger than what `magi-journal` does today, which flushes without `fsync`.
6. **Nothing is invented.** No default `at`, no synthesised id, no back-filled `kind` on a row
   that arrived without one, if any of it can reach `raw`.

## 4. What happens when balthasar is not there

This is the requirement with teeth, and it is a change in kind rather than degree. Today a
missing sibling costs a feature; `crates/magi-cli/src/melchior.rs:289` states the rule — a
sibling not being installed is the ordinary case, not a failure. Once balthasar is the only copy
that stops being true: no balthasar means no transcript, and a session that runs anyway is one
that cannot be resumed and does not know it.

Magi therefore needs balthasar to distinguish three things it can say, and to say them
differently:

- **refused** — a verb balthasar will not do. Magi carries on; this is not fatal.
- **unavailable** — no socket, no answer, dialled and nothing there. Magi must not start a turn.
- **failed to write** — `observe` did not return durable. The turn that just happened is not
  recorded, and magi has to stop rather than continue on top of a gap.

A dropped connection is not an answer to any of these. The family contract already says a
refusal is a reply rather than a closed socket; this extends it to the two failure modes that
only exist once the store is load-bearing.

## 5. Open, and worth settling before either side is built

**Blocks versus records.** `PLAN_SCROLLBACK.md` §3 says blocks share an `entry` id — "which
message this block is part of" — which implies an assistant message splits into a prose block and
a thinking block. But it also says `raw` is the serialised record. Both cannot hold: either one
row is one `Entry` and `entry` is redundant with the entry's own `id`, or a row is a fragment and
`raw` cannot be the whole record without storing it twice. The first is simpler and matches the
journal, which stores entries and not events precisely so a session file can be read with `less`
(`crates/magi-journal/src/record.rs:11-13`). Recommend one row per entry, and `kind` as a
projection of the entry's dominant content.

**Amendment traffic.** A streaming assistant message is amended on a cadence, and each amendment
is a `synchronous = FULL` write. Magi's own journal deliberately does not `fsync` per token
because it would be ruinous. Whatever cadence magi settles on, balthasar should not assume
amendments are rare.

**Who owns `at`.** Magi has the clock that matters for ordering within a turn, balthasar has the
one that matters for retention. Defaulting to balthasar's clock is fine as long as ordering never
consults it.

## What this does not cover

Memory — `remember`, `recall`, `forget`, the ladder from a session store into the project's. This
document is only the scrollback: what has to come back so the screen can be redrawn. The two
share a socket and nothing else.
