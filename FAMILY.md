# The family contract, revision 1

What every program in this family answers, so that a stranger can find out what it does without
reading its source — and so a fifth program can join by implementing a list rather than by
imitating four codebases.

This document is the canonical text. It is copied into each repository beside the code that
implements it, and `scripts/gate-family.sh` is the executable version: point it at a binary and it
reports which of this contract that binary implements.

---

## The reply shape

Every answer, on every door, in either encoding:

```json
{"ok": true,  "family": 1, "n": 2, "result": [ … ]}
{"ok": false, "family": 1, "n": 0, "result": [], "error": "…", "fault": "refused"}
```

| field | means |
|---|---|
| `ok` | whether the call was answered |
| `family` | which revision of this contract the reply is written in — **this document is 1** |
| `n` | how many values came back |
| `result` | the values, **always a list**, so one call answers with what the function did |
| `error` | why not, when `ok` is false |
| `fault` | which kind of no. Absent means `refused`. |

**A refusal is a reply, and its exit status is zero.** A real error arriving as "exited 1" cannot
be told from the binary being missing, and the two need different responses: one is a bug to
report, the other is a sibling to carry on without.

**A newer `family` is refused by name; an older one is not.** A reply with no `family` at all is
from before the field existed and is accepted.

**`verbs` also carries `surface`.** Two numbers, because they change for different reasons and a
consumer cares about different halves:

| | what it versions | who reads it |
|---|---|---|
| `family` | the wire between these programs — the reply shape, the encodings, which verbs exist | a sibling, or anything speaking to one |
| `surface` | what a plugin file is written against — the registrar names, the fields each declaration owes, what a callback is handed | a third party's extension |

Both go up only when something already published stops working. Adding a registrar, a field, an
event or a verb moves neither; renaming one, removing one, or changing what a field means moves
the one it belongs to. `surface` appears on `verbs` and nowhere else, because it is a fact about
the program rather than about the reply.

**`result` is the rows, and `n` is how many there are.** A verb that lists things puts each thing
in `result` as its own value; it does not put the whole listing in as one value that is a list.
`"result": [[…]]` with `"n": 1` is the mistake, and it is invisible from one side — casper sent
every listing it had that way while the other three sent theirs flat, and the coordinator reading
them one row at a time found an array where a declaration belonged and concluded casper declared
nothing at all. `gate-family.sh` checks this now.

---

## The floor — every program answers these

| verb | answers |
|---|---|
| `verbs` | every verb this program answers, each with one line saying what it does |
| `client` | this program's own client library, as source, on stdout |

`verbs` answers **in the reply shape above**, never as prose. A program's self-description is the
one thing another program must be able to parse; if it is human text then the only way to discover
a surface is to read it, which is the situation this contract exists to end.

`client` is served by the program that implements the surface, so a consumer cannot hold a stale
copy — the client comes from the server. `lua-api` is accepted as an older name for it.

**A program with no client library still answers.** casper is reached by spawn with a JSON call
and has no Lua-facing surface, so its answer is a refusal that says so. The contract requires the
question to be *answered*, not that every program have a library — and "there is none, because the
surface is reached this other way" is a parseable answer, where silence is not.

---

## Coordinated — every program a coordinator may configure

| verb | answers |
|---|---|
| `needs` | what a coordinator may tell it, as typed declarations |
| `configure` | takes that configuration on stdin, applies it, and reports per setting what was kept and what was refused |

A refused setting is **named**, with what would have been accepted:

```json
{"name": "nonesuch",
 "why": "melchior takes no setting by that name; `needs` lists what it takes"}
```

A coordinator is not required to answer these. magi coordinates and is not coordinated; the other
three are.

---

**A program that is one process per call is told on every spawn.** `configure` sets something in
the process that answers it, which is the whole of what a program needs when it is asked once and
then runs for the session — melchior and balthasar. casper is spawned per call, so a `configure`
sent to it would report the setting taken and every later call would be a fresh process knowing
nothing about it: a program answering the contract and doing nothing.

So casper reads `CASPER_CONFIGURE` — the same JSON object `configure` would have applied — from
the environment it was spawned with, and whoever spawns it sets it every time. One process, one
configuration: no state on disk for two sessions to fight over, and none to outlive the session
that set it.

`configure` still exists and still answers, because a coordinator wants to know *which* of its
settings would be refused before it commits to sending them. That is what the verb is for on a
spawn-per-call program: a dry run that names what it did not understand.

**A setting a program declares must change something.** A `needs` entry that nothing reads is the
same sin as a verb that is advertised and refused, one level down: a coordinator sets it, is told
it was taken, and the behaviour never moves. casper declared three and honoured one for a while —
`off` was dead code nothing called, and `output_bytes` was named in its own description and in a
test and nowhere else.


## Encodings

`--json` and `--cbor`, on every verb above. JSON is the default and is what a person reading a
pipe gets. CBOR is the same shape as bytes, for a caller that is not going to read it.

On a socket nothing is negotiated: a body says which encoding it is in its first byte — JSON's
top level is `{` or `[`, CBOR's map or array is `0x80`–`0xBF`, and the ranges do not overlap — so
a reply goes back in whichever the call arrived in.

---

## Extending one — the same directories everywhere

All four are Lua at the edges, and all four discover what is installed the same way. It is
neovim's runtimepath, unchanged, because twenty years of real plugins have been written against it
and most people arriving already know it:

```text
  <config>/plugin/*.lua                 alphabetical, each on its own
  <data>/<program>/site/pack/*/start/*/plugin/*.lua      installed packages
  <config>/after/plugin/*.lua           the last word
```

Where `<config>` is `$XDG_CONFIG_HOME/<program>` and `<data>` is `$XDG_DATA_HOME`. Each program
runs its own shipped declarations first, then this, then whatever a coordinator handed it — and
**every registrar replaces by name**, so the order *is* the precedence: `after/` is how a person
overrides something a package they installed declared.

Naming a file explicitly still works and is still the auditable case: magi's `magi.load`,
casper's `load` setting, melchior's `apis.lua` and `providers.lua`. Discovery is for what is
*installed*, because requiring an edit to somebody's own `init.lua` to enable a package makes
every package a merge conflict.

**A discovered file cannot spawn a process.** The sandbox is applied to the VM before any file
runs, in all four, so what arrives by being installed is held to the same rule as what arrives by
being named. What a *tool* runs is a separate question, and it goes through the declared runner.

**A file that raises costs itself and nothing else.** Somebody else's package failing is reported
and skipped; a program's own configuration failing is fatal, because a config that will not parse
has not expressed an intention.

Balthasar wrote this first and it lived there alone for a while, described as one program's
arrangement rather than the family's. The copies are copies on purpose — a shared crate between
these four is the dependency the whole arrangement exists to prevent.

---

## Installed packages — acknowledged, or they do not run

A file you put in your own `plugin/` directory runs on sight. A package under
`<data>/<program>/site/pack/` does not, until you have said it may:

```
<program> acknowledge
```

That writes `<config>/installed.json` — every installed file and the SHA-256 of what it held when
you agreed to it. A file whose digest does not match, or that is not in the manifest at all, is
**not run**, and the program says which one and what to type. Acknowledging replaces the manifest
rather than merging into it, so removing a package forgets it.

**Fail-closed, and only for what somebody else wrote.** Confirming your own configuration is a
prompt nobody reads — it trains people to say yes. A package is code that arrived by being fetched
and can change under you between one run and the next, which is the case where an acknowledgement
means something. A manifest that will not parse reads as empty, which holds everything back rather
than letting everything through.

This is the ten percent of a package manager worth having. Fetching is `git clone`; the idea is
the lockfile.

---

## Client libraries — from the program that implements them

`client` prints one plain-Lua file: what another program needs in order to talk to this one. A
consumer runs it and holds the result; it never keeps a copy of its own.

**Nobody vendors anybody's.** The library and the surface it talks to are one thing, and a copy
goes stale silently: magi shipped 631 lines of balthasar's, and a copy that had fallen behind
removed every memory tool from every session on a machine without saying anything. magi asks now,
at config load, and a sibling that is not installed lends nothing — which is right, because the
tools its library would declare could not have worked anyway.

A program whose surface is not reached from a Lua VM still answers: casper prints a refusal
saying so in as many words, because "there is none, and here is why" is parseable and silence is
not.

The libraries for `oslo` and `hexe` are the exception and the last copies left: neither answers
`client` yet, and both live outside these repositories. They are kept byte-identical wherever they
appear, which is the second-best thing to not copying them.

---

## Doors

A program may answer on its command line, on a socket, or both. **Where the two differ, the
difference is written down here**, so that a deliberate asymmetry is legible and an accidental one
cannot hide among them.

| program | command line | socket | deliberate differences |
|---|---|---|---|
| magi | the floor | one per session, its own CBOR protocol for a front end | Coordinates rather than being coordinated: no `needs`, no `configure`. |
| casper | the floor, plus `tools`, `run`, `surface` | read-only verbs only | **`run` is never reachable over the socket.** casper's job is running programs, and a socket that runs commands is a remote shell wearing a friendly name. The spawn link carries the trust instead. |
| melchior | the floor, plus `models`, `ask`, `serve`, `fork`, `auth` | the session surface | — |
| balthasar | the floor, plus its own memory verbs | the same memory verbs | Some verbs are owner-only: a socket peer may propose but may not pin, write globally, or purge, and may not sign its report as the user's judgment. |

---

## Two rules that are tested, not reviewed

**Advertised equals dispatched.** Every verb `verbs` lists is one the program answers, and every
verb it answers is one `verbs` lists. A surface that advertises what it refuses is worse than one
that advertises nothing, because the second is honest.

**The client library round-trips.** What `client` prints is what the program's own surface accepts.

---

## Joining the family

Implement the floor, answer in the reply shape, and pass `gate-family.sh`. Nothing else is
required — there is no registration, no manifest, and no shared library to link, because a shared
library between these programs is the dependency the whole arrangement exists to prevent.
