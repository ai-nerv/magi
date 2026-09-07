# Extending any of them

One page. It is the same shape in all four programs, so what you learn adding a tool to magi is
what you already know when you add a wire protocol to melchior.

The rule underneath everything here: **the last declaration of a name wins.** Every registrar
replaces by identity, so the load order *is* the precedence, and `after/plugin/` is how you
override something you did not write.

---

## 1. Where the file goes

```
~/.config/<program>/plugin/yours.lua                          alphabetical, each on its own
~/.local/share/<program>/site/pack/*/start/*/plugin/*.lua      installed packages
~/.config/<program>/after/plugin/yours.lua                     the last word
```

`<program>` is `magi`, `casper`, `melchior` or `balthasar`. Make the directory if it is not
there. Nothing else needs telling: there is no registration and no entry point to edit.

**A file under `site/pack/` is one exception.** Your own `plugin/` files run on sight; a package
you fetched runs once you have said it may:

```
<program> acknowledge
```

That records the SHA-256 of every installed file in `<config>/installed.json`. One that is not in
the manifest, or whose digest no longer matches, does not run -- and the program says which and
what to type. Run it after installing or updating anything.

That last part is the point. Requiring an edit to your own `init.lua` to enable somebody's
package makes every package a merge conflict with your configuration.

Working examples live in each repository under `examples/plugin/`. They are run by that
repository's test suite, so an example that does not load fails the build rather than wasting
your afternoon.

**casper's thirteen are compiled into the binary**, and its config directory layers over them
rather than replacing them. `~/.config/casper/tools.lua` runs after the shipped ones, so a file
declaring `cat` means it and a file declaring something new adds one. That is why `make install`
copies nothing there: a copy of the shipped declarations beside the binary would win over the
binary's own, and you would be pinned to whatever `tools.lua` shipped the day you installed.

To switch one off without writing any Lua, tell casper from magi's config:

```lua
magi.casper = { tools = { dino = { off = true }, birdy = { hidden = true } } }
```

`off` removes it entirely; `hidden` keeps it runnable and takes it out of what the model is shown.
magi passes this on every casper spawn, because casper is one process per call — see `FAMILY.md`.

---

## 2. What you may do in one

A plugin runs in the program's own Lua VM, with the sandbox on. `os.execute` and `io.popen` are
gone; so is `io` entirely, along with `require`, `dofile` and `loadfile`. That is not "a config
must never run anything" — it is that spawning outside the seam happens with nothing checking the
path, nothing asking the person, and nothing bounding what comes back.

What you use instead:

| program | to run something | to touch a file |
|---|---|---|
| magi | `magi.shell(command)` — gated, and only inside a tool's `run` | `magi.fs.write(path, text)`, `magi.fs.ls(dir)` |
| casper | `casper.exec(program, { args })` — a list, so there is no shell in between | its declared tools |
| melchior | — | `melchior.fs` |
| balthasar | — | `balthasar.fs` |

`magi.shell` and `magi.fs.write` go through the same permission gate the built-in tools do: the
person is asked, the answer is remembered, and `magi.confine` applies. Both answer `nil, why`
rather than raising, so a refusal is something to handle rather than something that breaks a turn.

A file that raises costs itself and nothing else. Your package failing is reported on stderr and
skipped; the program's own configuration failing is fatal, because a config that will not parse
has not expressed an intention.

---

## 3. The registrars

### magi — a tool

```lua
magi.tool("name", {
  description = "what it does, in the model's terms",
  parameters = { type = "object", properties = { … }, required = { … } },
  transport = { kind = "lua" },       -- the body is the `run` below, in this VM
  needs = "run",                      -- read | write | run | reach; omit if it touches nothing
  run = function(args)                -- return { content = … } or { content = …, is_error = true }
    …
  end,
})
```

`transport` is not optional and there is no default: a tool with a `run` and no transport is
refused at load with "missing field `transport`", because the registry has no way to guess that
the function is the point. The other kinds are declarations rather than code — `command` spawns
one program per call with the arguments in argv, `casper` hands the call to casper.

### magi — a watcher

```lua
magi.watch("name", { run = function(event) … end })
```

Told after the fact, answered with nothing: a watcher cannot change a result and cannot fail one.
Branch on `event.kind` and nothing else — a watcher written when there was one kind of event
assumed every event had a `tool` field, and that is now wrong.

| `kind` | carries |
|---|---|
| `session.opened` | `id`, `resumed` |
| `turn.began` | `model` |
| `turn.ended` | `model`, `took_ms`, `ok` |
| `tool.finished` | `tool`, `arguments`, `is_error` |
| `permission.asked` | `verb`, `about` |
| `permission.answered` | `verb`, `about`, `allowed` |
| `context.compacted` | `dropped`, `kept` |
| `provider.retried` | `mind`, `attempt`, `of`, `delay_ms` |

### casper — a tool

```lua
casper.tool("name", {
  description = "…",
  parameters = { … },
  needs = "read",
  run = function(args)                -- { said = … } or { said = …, failed = true }
    local done = casper.exec("jq", { args.filter, args.path })
    …
  end,
})
```

`said` is what the model reads and should be plain. `shown` is what the person sees and may carry
colour — and a tool never names a colour, it names what its output *means* (`added`, `keyword`,
`path`) and the harness resolves that against its own palette.

### melchior — a provider or a wire protocol

```lua
melchior.provider("id", { name = …, api = …, base_url = …, auth = …, models = { … } })
melchior.api("id", { endpoint = …, headers = …, request = …, on_event = … })
```

`api` names a protocol, not a vendor. `melchior.apis` is the registry and is readable, so a
dialect that differs from a shipped one in one function borrows the other three rather than
forking eight hundred lines.

### balthasar — a source or a section

```lua
balthasar.source("id", { sessions = …, meta = …, line = … })
balthasar.section("id", { weight = …, order = …, tiers = …, where = …, render = … })
```

A source answering `nil` means "not one of ours", which is what keeps a glob that catches a
neighbouring program's file harmless.

---

## 4. What a project file may not do

A `.magi.lua` or `.balthasar.lua` arrives with a checkout, and cloning a repository must not be a
way to run code. Those files may *choose* among what your own configuration offers; they may not
declare a command to run, an endpoint to send text to, or how your transcripts are read.

`magi.trusted = { "/home/you/work" }` in your own configuration lifts that for directories you
vouch for, ancestors included. It is a decision that belongs to a person, made once, in the file
only they can edit.

---

## 5. Talking to another one of them

Every program prints its own client library:

```
balthasar client > /dev/null       # plain Lua, ready to load
melchior client
magi client                        # `lua-api` is the older name for the same thing
casper client                      # a refusal: casper is reached by spawning it with a call
```

Do not keep a copy. The library and the surface it talks to ship together, and a copy that has
fallen behind fails quietly — magi kept 631 lines of balthasar's, and a stale one removed every
memory tool from every session on a machine without saying anything.

Inside magi the siblings' libraries are already loaded and available to a tool as it declares
itself, so this is only for programs outside the family.

---

## 6. Checking it

```
<program> verbs            what it answers, on each of its doors
<program> needs            what a coordinator may set
<program> acknowledge      clear the installed packages, and list what it took
magi doctor                what a session actually composed, without starting one
```

`FAMILY.md` is the contract these implement, and `gate-family.sh` is what holds them to it.

## 7. What is stable

`<program> verbs` reports `surface`, which is the version of everything in section 3. It is **1**
in all four.

It goes up only when something already published stops working. Adding a registrar, a field or an
event does not move it — a file written against 1 keeps running. Renaming one, removing one, or
changing what a field means does. If your plugin cares, read it and refuse rather than failing
halfway:

```lua
-- at the top of your file
local surface = 1  -- what this was written against
```

Stable at surface 1:

| | |
|---|---|
| magi | `magi.tool`, `magi.watch`, `magi.load`, `magi.shell`, `magi.fs`, `magi.json`, `magi.stream`, and the eight event kinds |
| casper | `casper.tool`, `casper.exec`, `casper.paint`, `casper.theme`, and `said` / `shown` / `failed` |
| melchior | `melchior.provider`, `melchior.api`, `melchior.apis`, `melchior.json`, `melchior.fs`, and the four functions a protocol owes |
| balthasar | `balthasar.source`, `balthasar.section`, `balthasar.json`, `balthasar.fs`, `balthasar.load`, and the three functions a source owes |

Not stable, and will change without the number moving: anything not named above, the exact wording
of a description, and the contents of the shipped configuration.
