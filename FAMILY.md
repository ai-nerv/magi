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

**A verb this program does not have is a refusal like any other.** It is the first thing any
caller does to a sibling it has not met, and it is where the rule above is broken most easily: an
argument parser that rejects an unknown subcommand writes usage to stderr and exits 2 all by
itself, and the program never sees the call. That is the "exited 1" case exactly — the caller
cannot tell it from a binary that is not installed. The unknown verb comes back on **stdout**, in
the reply shape, at exit 0, naming what was asked for.

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

**Every verb takes both, including the ones whose bare output is not a reply.** `client` prints
its library as source, because that is what a person redirecting it into a file wants; asked with
`--json` or `--cbor` it comes back framed, with the source as the single value in `result`. A verb
that rejects the family's own encoding flag is refusing the contract, and refusing it the worst
way — with an argument parser's error, on stderr, at exit 2.

On a socket nothing is negotiated: a body says which encoding it is in its first byte — JSON's
top level is `{` or `[`, CBOR's map or array is `0x80`–`0xBF`, and the ranges do not overlap — so
a reply goes back in whichever the call arrived in.

---

## A caller that abandons a call abandons the connection

**A reply says nothing about which call it answers.** There is no request id in the shape above,
and adding one would move `family` for every program at once. What holds the two ends in step is
position: one reply per call, in the order the calls were made.

So a caller that gives up on a call — a timeout, a cancelled future, a `recv` that returned an
error — has left a request on the wire whose reply is still coming. **It may not make another call
on that connection.** The abandoned reply arrives first and is read as the next call's answer, and
every answer after that belongs to the call before it. A write is told it landed by somebody else's
reply; a verb is refused with an error it cannot produce.

That is not hypothetical and it is not cheap to find. magi timed out a first call against a
balthasar that was still opening its store, kept the connection, and then read the `plan` verb's
refusal as the answer to `observe` — in 96 microseconds, from a call it had never made. The
transcript it believed it had written was dropped, and `--resume` came back empty. It read as a
flaky test for weeks.

The rule is one line to obey: **on any failure to send a whole call or read a whole reply, close
the connection.** Dial again for the next call; a connect is cheaper than a conversation that is
quietly one behind.

The rule binds a client that **holds its handle across calls**, which is what a program asked
several times a turn will want to do. Such a client must close at *every* point a read can fail,
including the one that has already consumed a frame header and would otherwise desynchronise on a
partial frame. A client that dials per call is already in step by construction — the connection
dies with the call it was made for — and needs nothing. Both shapes exist in this family and both
are correct; what is not correct is holding a handle and keeping it through a failure.

**The send half is the same fault seen from the other end.** A write that fails partway has put
the head of a call on the wire that this side will never finish, and the far end reads whatever
comes next as the rest of it. The reply to that half-call, if one comes, answers nothing that was
asked. Both directions close.

**Closing means the handle is gone, not merely shut.** The next call must meet "this connection is
closed" and not a nil handle, a reused file descriptor, or a second error from the transport — a
caller that cannot tell a closed line from a broken one will retry on the wrong one.

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

**A file that arrived with a checkout is data, not a program.** A project's own file —
`.magi.lua`, `.balthasar.lua`, anything a repository carries for one of these programs — runs in
a VM of its own with no socket, no filesystem, no environment and no `load`, under an instruction
budget, and may set only the settings its program lists as a project's to set. Everything else it
assigns is refused by name, and the refusal is fatal to the session that read it. A directory the
person has named as trusted in their own configuration is the one exception, and only the person's
own file can name it.

**Every VM runs under a budget.** A configuration, a hook or a plugin that loops is stopped and
reported as failing; it never stalls the program that ran it.

**A socket a VM may dial is one of the family's, by name.** A program's Lua stream reaches the
sockets of these four programs in their runtime directories and nothing else — not "anything under
the runtime directory", which is also where the session bus, the agents and the user's systemd
listen. The one addition is casper's: it lends clients for hexe and oslo, so its VM also reaches
`hexe` and `onix` by the same rule.

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

A program may answer on its command line, on a socket, on the door a harness calls for a model, or
on any of them. **Where they differ, the difference is written down here**, so that a deliberate
asymmetry is legible and an accidental one cannot hide among them.

Every row `verbs` returns carries a `door`, and it is one of three words: `cli` for the command
line, `socket` for a bound socket, and `tool` for the surface a harness calls on behalf of a model
— reached in melchior as `melchior tool --verb X`, one exec per call. A verb reachable on more
than one door is listed once per door. The field is what tells a caller where to knock, and it is
checked rather than trusted:

**A program must not name a door it cannot open.** A verb advertised on `socket` by a program with
no way to start one is advertised-and-refused wearing a label that hides it — the probe that would
catch it on the command line skips it as correctly-absent, and the one that would catch it on the
socket can never connect. So a program with any `socket` verb answers `serve` on its command line,
and a program with any `tool` verb answers `tool`, because that is the only thing that opens the
door it is claiming.

`tool` is the door a gate cannot probe: it answers for a live session and a gate has none. What is
checked from outside is that the door exists; that every verb behind it is dispatched is held by
the program's own tests.

| program | command line | socket | deliberate differences |
|---|---|---|---|
| magi | the floor | one per session, its own CBOR protocol for a front end | Coordinates rather than being coordinated: no `needs`, no `configure`. |
| casper | the floor, plus `tools`, `run`, `surface`, `serve` | `tools` and `run`, kept open across a session | What `run` runs, it runs inside the jail `serve` was spawned with — the one the coordinator set in casper's environment, never one a call named — and only for a caller presenting the key the coordinator handed `serve` on its standard input. A peer of another user is turned away. With the jail off the socket runs commands as the person, which is what the person chose; it is a remote shell exactly as far as the person's own shell is. The command line stays the spawn-per-call door; the socket is the same surface, reached without a spawn each time. |
| melchior | the floor, plus `models`, `ask`, `serve`, `fork`, `auth` | the session surface | The only one with a `tool` door: the coordination vocabulary a model calls, reached as `melchior tool --verb X`. `identity` and `tell` on the socket are `whoami` and `send` there, and are not aliases — one answers a program with a record, the other a model with a paragraph. |
| balthasar | the floor, plus its own memory verbs | the same memory verbs | Some verbs are owner-only: a socket peer may propose but may not pin, write globally, or purge, and may not sign its report as the user's judgment. A call on the socket that writes or issues work is answered only with the key `serve` was started with: the one the coordinator handed it on its standard input or, handed none, one it mints and writes beside its socket as `<socket>.key` (0600, in its 0700 runtime directory, removed when it exits) for whoever started it. A call that only reads needs no key. It records for the agent `serve` was spawned for, never one a caller's environment names. `export` and `dataset` answer `--json` as one object per line rather than framed: they are a corpus, not an answer, and `import` reads back what `export` writes. |

---

## Frames

On a socket, every call and every reply is one frame: four bytes of length, big-endian and
unsigned, then that many bytes of body in one of the two encodings above.

**A frame is at most 16 MiB (16 777 216 bytes), in all four, in both directions.** One number,
because a writer that allows more than its reader is a connection that closes for no reason the
writer can see — balthasar read at 8 MiB while magi wrote at 16, and a pasted prompt silently
stopped a session being recorded. Every reader checks the declared length before allocating for
it. Every writer checks before sending a byte: a call that would not fit is refused locally and
never sent.

**A frame of length zero is malformed**, answered with a refusal like any body that will not
decode.

**An oversized call is answered, not dropped.** A reader that meets a declared length over the
cap answers one refusal — `fault: "failed"`, naming the cap — and then closes, because it cannot
skip a body it will not read and stay in step. A reply that would be over the cap is never
written: the program answers that call with the same refusal instead, and the connection stays in
step. A caller can always tell "too large" from "the program died".

**A row over the cap is split, never a reason a stream cannot continue.** A verb that pages
returns at least one row per page only when that row fits; one that does not is returned
truncated and marked so, and the next page starts after it.

---

## Where sockets live

**The runtime directory is trusted only when it is safe.** `$XDG_RUNTIME_DIR` is used when it is
an absolute path to a directory owned by this uid with no group or other permissions. Empty is
unset. Otherwise each program uses `<tmp>/<program>-<uid>`, where `<tmp>` is the system temporary
directory. Every directory a program creates for its sockets, notes or settings is created mode
`0700` and then checked with `lstat`: a directory, not a symlink, owned by this uid, with no group
or other permissions. A check that fails is a refusal naming the path, never a fallback to
somewhere less safe.

**A stale socket is one nothing answers.** A program unlinks a socket path only when connecting to
it fails with `ECONNREFUSED` and `lstat` says it is a socket owned by this uid, and only while it
holds an exclusive `flock` on `<socket>.lock`, which it keeps for as long as it listens. A timeout,
a full backlog, `EAGAIN`, `EMFILE` or a refusal reply is a live program having a bad moment, not a
corpse. On exit a program unlinks its socket only if the path is still the inode it bound.

**A client checks who answered.** After connecting, a client reads the server's `SO_PEERCRED`
and closes if the uid is not its own.

---

## Deadlines and limits

**Every connection has deadlines, on both sides.** A server stops waiting for the rest of a frame
30 seconds after its first byte, closes a connection idle for 5 minutes, and gives up on a write
after 30 seconds. A client puts the same deadline on writing a call as on reading its reply. A
deadline that passes is a failure to send or read a whole frame, and the rule above applies: the
connection is closed.

**A busy server says so.** A program at its limit of connections answers the next one with a
single refusal — `fault: "failed"`, error beginning `busy` — and closes it. It does not drop the
connection silently, and it does not stop listening.

**An accept that fails is a moment, not an ending.** `EMFILE`, `ENFILE`, `ENOBUFS` and
`ECONNABORTED` are logged and retried after a pause; the listener keeps listening.

**A child process has a deadline too.** A program that waits on another — a sibling's `models`,
a distiller, a tool — waits with a deadline, kills what it started when the deadline passes or the
call is cancelled, and reaps it.

---

## Who may call

**The uid is the boundary between people. Within one uid, the jail is the boundary.** A command a
tool runs inside the jail reaches no family socket: the runtime directory is not in its mount
namespace, the abstract socket namespace is scoped away, and on the path without bubblewrap it
cannot open a Unix socket at all. Every socket it opens is IPv4, IPv6, routing netlink or, under
bubblewrap only, Unix; any other address family is refused. Everything unjailed and running as
this uid could edit the person's own configuration, and no protocol here pretends otherwise.

**Who a caller is, read from its environment, is attribution and never authority.** A program may
read a peer's `/proc/<pid>/environ` to learn which session a call comes from, once, when the
connection is accepted. It never falls back to a name the call itself carries. A verb that grants,
stops, adopts, hands over or controls requires a secret minted for that relationship.

**A secret never travels in an environment variable, an argument or a log.** It is handed over on
an inherited pipe or in a `0600` file inside a `0700` runtime directory, read once, and redacted
wherever a call is logged.

**Grants only narrow.** A grant handed from one session to another is intersected with what the
receiving session already holds, never added to it.

---

## Environment that carries capability

A variable on this list changes what a program may do. Each program removes every one of them from
the environment of anything it starts that is not a member of this family, and a jailed command
receives none of them.

| variable | set by | read by | what it carries |
|---|---|---|---|
| `CASPER_JAIL` | magi | casper | the jail a tool call runs in |
| `CASPER_CONFIGURE` / `MAGI_TOOLS_CONFIGURE` | magi | casper | casper's settings for one spawn |
| `MAGI_GRANTS` | magi, for a forked child | magi | grants that narrow the child's own |
| `MAGI_NO_SPAWN` | magi | magi | that a session may not start another |
| `MAGI_MELCHIOR_PROJECT` / `_ID` / `_ROLE` / `_PARENT` / `_SESSION` | melchior, magi | melchior | which session a process belongs to (attribution) |
| `MAGI_MEMORY_AGENT` / `BALTHASAR_AGENT` | magi, on `serve`'s spawn | balthasar | which agent a `serve` records for; in a caller's environment, attribution only |
| `NERV_GIVEN` | magi | balthasar, melchior | the per-session directory of settings a coordinator gave |
| `MAGI_API_SOCKET` | the person | magi | a balthasar to use instead of starting one |
| `MAGI_HOST_SOCKET` / `MAGI_PARENT_HOST_SOCKET` | magi | magi | where a session, or its parent, answers |
| `MAGI_SESSION_PID` | magi | magi | the session a child watches |
| `MAGI_COORD_PROGRAM` | magi, from the person's configuration | magi | the program that fills the coordination role |
| `MAGI_TOOLS_DOOR` / `MAGI_MEMORY_DOOR` / `MAGI_AGENT_DOOR` | the person | magi | which door reaches a sibling |
| `*_DEBUG_LOG`, `NERV_LOG` | the person | all four | where a log is appended, created `0600` |
| `MELCHIOR_CONFIG` | the person | melchior | another configuration file |
| provider keys | the person | melchior | credentials, which only melchior reads |

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
