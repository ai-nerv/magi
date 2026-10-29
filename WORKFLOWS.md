# Workflow skills

A workflow is a named Markdown document selected with `/skill`. Magi reads it
through Casper, adds its body to that turn's model context, and stores the selected
body and provenance in Balthasar. It is not a Lua plugin, executable configuration,
an automatically learned procedure, or a grant of permission.

## A first workflow

Create `.magi/skills/review/SKILL.md` under the directory where Magi is launched:

```markdown
---
name: review
description: "Review changes: correctness and tests"
---
Review the requested files for correctness and missing tests.
Explain findings before proposing edits.
```

In the prompt, type `/skill review src/lib.rs`. `/skill ` offers completion;
`:skills` refreshes its catalog. Completion loads names and descriptions, not all
workflow bodies. Invalid metadata and duplicate names produce diagnostics.

The same host operation is available in print mode:

```sh
magi -p '/skill review src/lib.rs; $(echo literal)'
```

Keep the shell quotes: Magi receives `src/lib.rs; $(echo literal)` as literal
arguments. Magi does not substitute variables, evaluate shell syntax or expand
templates in workflow text. Arguments are appended to the selected body as a
JSON string labelled as data; they do not replace placeholders in the body.

Only `/skill` at the start of the prompt followed by whitespace or end-of-input
uses this operation. `/skills`, other slash words and leading-space text remain
ordinary prompts. Bare `/skill`, invalid names, and missing workflows are refused
visibly. One whitespace character after the name is consumed; remaining argument
bytes, including further whitespace, are preserved. Print mode exits nonzero on
a workflow refusal rather than silently submitting an ordinary prompt.

## Locations and precedence

`WORKSPACE` is the launch directory by default. `CONFIG_HOME` is the user's XDG
configuration home, normally `$XDG_CONFIG_HOME` or `$HOME/.config`.

| Priority | Root |
|---|---|
| 1 | `WORKSPACE/.magi/skills` |
| 2 | `WORKSPACE/.agents/skills` |
| 3 | `CONFIG_HOME/magi/skills` |

Each root contributes only immediate `<name>/SKILL.md` entries. There is no
ancestor search, Git-root search, recursive discovery, or scan of other tools'
user directories. Launching in a nested directory does not select an ancestor's
workflows, including in a linked Git worktree.

Set `magi.workflow_workspace = "/absolute/workspace"` in your trusted Magi Lua
configuration to select another project root. A relative value is resolved from
the launch directory; `nil` leaves the launch default. Parent traversal and unsafe
paths are refused, not replaced with a fallback root. Untrusted project Lua
configuration cannot set this option. It changes only the two project workflow
roots, not the user root, tool working directory, or permissions.

Names are sorted. The first root containing a name wins, even if that entry's
metadata is invalid; later duplicates generate warnings, not fallback selections.
Missing roots are empty. An unreadable, unsafe, or over-limit root fails discovery
instead of silently substituting another location.

## File format and bounds

| Item | Limit |
|---|---|
| Entries per root | 256 |
| Metadata, including delimiters and line endings | 4096 bytes |
| Complete selected file, including metadata | 65536 bytes |

Limits are inclusive and count bytes, not characters. Invalid root entries count
toward the entry limit. Sources must be regular, single-linked UTF-8 files without
NUL bytes. Symlinked paths, hardlinked files, pipes and other special files are
refused. Keep the source stable during a read; detected replacement or mutation
fails the acquisition.

The directory and `name` must match exactly. Names contain 1–64 lowercase ASCII
letters, digits or hyphens and must start and end with a letter or digit. No case
folding or Unicode normalization is performed on identifiers. Descriptions and
bodies may contain Unicode.

The header starts and ends with a line containing exactly `---`. It contains only
one `name` and one `description` key, both nonempty scalar strings without control
characters. Blank lines and whole-line comments beginning with `#` are accepted.
Values may be plain, single-quoted with doubled apostrophes, or double-quoted with
JSON string escaping. Quote punctuation such as `: ` or ` #` inside a value.
Unknown or repeated keys, YAML nesting, block scalars, tags, anchors, aliases and
inline comments are not supported. The body must contain non-whitespace text.

## History and authority

Selection reads the complete file and computes SHA-256 over its exact bytes,
including metadata. The transcript shows the selected root and full digest.
Accepted history retains that body's bytes, source, digest and literal arguments;
resume and rewind use the stored selection, not a changed or deleted source file.
A later invocation reads the source again. If metadata changes between discovery
and selection, refresh the catalog before selecting it again.

Workflow records that cannot fit the prompt or lossless history/replay bounds are
refused; they are not silently truncated or converted into ordinary text. Older
Magi binaries that do not understand workflow records cannot explicitly resume
those runs. Keep a compatible binary for workflow history.

References in the body do not trigger eager file reads, downloads or script
execution. Any model-requested tool use still goes through the usual permissions
and jail. A body claiming that commands are preapproved has no such authority.
External body text is not original user evidence for pinned memory rules.

## Current verification limits

Native acquisition, replay, Landlock and bubblewrap permission and memory-authority
checks have passed, together with full-family verification and deterministic acceptance.
Command and persistent-socket permission fixtures verify a readable protected file
at a stable system path, denied writes, explicit permission decisions, and replay.
Real-terminal completion, provenance, scrolling, missing/invalid-workflow and
interruption recovery checks pass at 80x24 and 160x28. Invalid source selection
makes no model call. Unreadable files and roots fail visibly without selecting a
readable duplicate; restoring permissions permits a fresh selection. Each helper
request runs under a ten-second deadline that includes
waiting for a worker, executable verification, spawn, exchange and reply decoding.
Cancellation or dropping the caller signals the worker without blocking the session.
At most four acquisition workers run per process; a worker retains its slot until
it finishes cleanup, even after its caller has timed out. Executable hashing streams
through a 64 KiB buffer. A blocked operating-system operation may outlive the caller
and retain capacity; no hard kill or process-shutdown deadline is promised for
uninterruptible kernel I/O. Queued prompt preparation has a separate 45-second
deadline covering discovery, selection and admission.

Run `oslo make test-skills-reference` for the local reference examples, root and
limit table checks, and command-help coverage. This does not build the website or
replace native reader, lifecycle, or real-terminal acceptance tests.
