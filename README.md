# axum

A coding agent for Linux.

Tau's bones, Pi's face: the system is a constellation of POSIX processes over Unix sockets,
and the terminal experience is Pi's — differential rendering into native scrollback, live
streaming, an editor-grade prompt.

## Status

**M0 — the UI.** The interface is built and driven by a recorded event stream; there is no
model, no agent loop, and no tools yet. See `PLAN.md` for the milestone plan.

```sh
make run          # the UI, against a replayed session (alt screen)
make run --inline # the same, letting the terminal keep the history
make build        # the release binary, static where the toolchain allows it
make install      # into $PREFIX/bin
make verify       # fmt, check, test, clippy, gates, docs
```

Two backends, one renderer. `alt` takes the alternate screen and owns the transcript, which is
what transcript search and selection will need; `inline` keeps a live region at the bottom of
the normal screen and lets the terminal keep the history, so native scroll, search, and copy
keep working. Both draw from the same components and answer the same keys, and the footer names
whichever is active.

`make run` starts a replay host and attaches the UI to it over a Unix socket — two processes,
as the architecture intends, even for a demo. To drive them separately:

```sh
make host         # a replay host alone
make ui           # the UI alone, attaching to it
```

## Layout

| Crate | Role |
|---|---|
| `axum-proto` | the wire contract: events, commands, envelope. No I/O |
| `axum-ipc` | Unix socket transport, length-prefixed CBOR, `SO_PEERCRED` identity |
| `axum-tui` | rendering: theme, markdown, transcript, editor, status, footer |
| `axum-cli` | the UI process |
| `axum-testkit` | fake harness and recordings |

## Development

`.make.lua` is the task interface; `make` on its own lists every recipe.

```sh
make test         # the suite
make gates        # the architectural gates
make clippy       # warnings denied
make verify       # all of it
```

The gates are not advisory:

| Gate | Rule |
|---|---|
| `gate-file-size` | no `.rs` over 800 lines |
| `gate-proto-size` | `axum-proto` under 4,000 lines |
| `gate-reachable` | no crate unreachable from the binary |

The gates are not advisory. They exist because Pi carries ~20,000 lines that nothing reaches
and a 6,549-line god file, and Tau has a 34,875-line one — each of which arrived one
reasonable commit at a time.

## Releases

`make dist` builds against `x86_64-unknown-linux-musl`, which produces a genuine `static-pie`
binary — no interpreter, no `NEEDED` entries, nothing to install alongside it:

```
axum 0.1.0   2.15 MB
binary  target/x86_64-unknown-linux-musl/release/axum
size    2.15 MB   2,249,400 bytes
linking ✓ static   no runtime dependencies
```

The linkage line is read out of the ELF, not inferred from the flags: a glibc "static" build
still carries an INTERP and dies on a machine whose loader disagrees, and `ldd` reports it as
statically linked anyway.

The release profile is pinned rather than left to defaults — `lto = "thin"`,
`codegen-units = 16` — because a build that silently switches to fat LTO and one codegen unit
turns a ten-second link into minutes.
