<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="misc/magi_alt-dark.svg">
    <source media="(prefers-color-scheme: light)" srcset="misc/magi_alt.svg">
    <img src="misc/magi_alt.svg" alt="magi" width="520">
  </picture>
</p>

<p align="center"><em>A coding agent for Linux.</em></p>

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
| **magi** | this one: the harness — UI, host, providers, tools |
| **melchior** | the agent layer — sessions talking to sessions, adoption, permissions |
| **balthasar** | the memory layer — what was said, distilled and recalled |
| **casper** | tooling and tool APIs. Not yet divided out |

**They are separate programs, not components.** melchior does not know what a harness is, and
balthasar's Rust never parses magi's types — each one is useful, and testable, with the others
absent. magi with no melchior is a session with no siblings, which is the ordinary case and not
an error.

## Status

A real agent: a model answers, tools run in their own processes, and the session is journalled
and resumable. See `PLAN.md` for how it was built.

```sh
make run          # magi, for real, in the current directory
make install      # the static binary into $PREFIX/bin, so `magi` works anywhere
make configs      # install config/ into ~/.config/magi, ready to edit
make build        # the release binary, static where the toolchain allows it
make verify       # fmt, check, test, clippy, gates, docs
```

**Both are needed.** `make configs` installs configuration; it does nothing for a binary you
have not installed. `make configs` says so when the two are out of step.

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

## Talking to other sessions

With `melchior` installed, a session can reach the other sessions in the same project. The
model calls one `agent` tool — `list`, `send`, `inbox`, `reply` — and magi's whole knowledge of
the layer is one file, `magi-cli/src/melchior.rs`, that spawns it and reads lines.

Two walls decide who can be reached. The **project** wall is the filesystem: another checkout's
sessions are not refused, they are simply not there. The **instance** wall is the front door: a
main speaks for its instance, and its subagents are private, so `agent_talk` is `mains` by
default and `instance` or `project` when you mean otherwise.

A message carries a **sort**, and the sort decides what it may interrupt. `question`, `answer`,
`attention`, `trouble` and `handoff` wake an idle session; only `attention` and `trouble` may
reach one mid-turn. Anything arriving during a turn waits, and the whole waiting room is
answered together by one turn at idle — so ten notes cost one reply, not ten.

One main may ask another to **adopt** it. Consent is a person's: the request surfaces as a
prompt on the other side, and accepting hands down exactly the grants the parent already holds
— never more. A child that wants something outside them is refused and told to ask its parent.

## Layout

| Crate | Role |
|---|---|
| `magi-proto` | the wire contract: events, commands, envelope. No I/O |
| `magi-ipc` | Unix socket transport, length-prefixed CBOR, `SO_PEERCRED` identity |
| `magi-model` | the provider-neutral message model |
| `magi-provider` | the HTTP side: streaming, SSE, retries, what each error means |
| `magi-core` | the turn loop, as an explicit state machine |
| `magi-tools` | what a tool is, and the three the floor is made of |
| `magi-lua` | the Lua VM, and the config API it offers `init.lua` |
| `magi-journal` | an append-only session journal |
| `magi-host` | the session: the journal, the socket, and the turns |
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

`gate-modules` earns its place on its own: a file nobody declares is not a compile error, not a
warning and not run — it simply is not part of the crate. Two were found at once, each holding
tests that had silently not run since the commit that moved them.

The gates are not advisory. Every agent this one was measured against carries dead code nothing
reaches and a god file in the tens of thousands of lines — 6,549 in one, 34,875 in another. None
of that was decided; it arrived one reasonable commit at a time, which is the only way it ever
arrives, and the reason the limit has to be a script rather than an intention.

## Releases

`make dist` builds against `x86_64-unknown-linux-musl`, which produces a genuine `static-pie`
binary — no interpreter, no `NEEDED` entries, nothing to install alongside it:

```
magi 0.1.0   2.15 MB
binary  target/x86_64-unknown-linux-musl/release/magi
size    2.15 MB   2,249,400 bytes
linking ✓ static   no runtime dependencies
```

The linkage line is read out of the ELF, not inferred from the flags: a glibc "static" build
still carries an INTERP and dies on a machine whose loader disagrees, and `ldd` reports it as
statically linked anyway.

The release profile is pinned rather than left to defaults — `lto = "thin"`,
`codegen-units = 16` — because a build that silently switches to fat LTO and one codegen unit
turns a ten-second link into minutes.

## Configuration

Everything magi knows about the outside world is Lua, and it all lives in `config/`:

| | |
|---|---|
| `config/apis/*.lua` | the wire protocols — how to talk to an endpoint |
| `config/providers.lua` | the catalog — which endpoints exist and what they offer |
| `config/tools.lua` | what the model may call, and how each tool is reached |
| `config/clients/*.lua` | the stubs siblings ship, copied in — `hexe` and `oslo` so far |
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
