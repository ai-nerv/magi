-- axon's build, as recipes. This replaced the Makefile; there is no other.
--
--   make            the recipes, with what each of them says it does
--   make build      the binary
--   make test       the suite
--   make verify     the whole local gate
--
-- At an oslo prompt in this directory `make` is enough; everywhere else it is `oslo make`.
-- CI has no oslo, so it calls the language's own tool -- nothing here is on the release path.
--
-- **Everything here builds `--release`.** `build` makes a release binary, `run` runs it and
-- `install` copies it, so a `check` or a `test` against the debug profile compiles the whole
-- workspace a second time into a second target directory -- and then verifies a set of
-- artefacts nobody is going to run. One profile, one set of artefacts, one thing verified.
-- `make debug` is the one way out, which is what its name is for.

local make = oslo.make

-- Name and version live in PROJECT, one per line, so every tool reads them from one place.
local function project()
  local found = {}
  for line in (oslo.fs.read("PROJECT") or ""):gmatch("[^\n]+") do
    local value = line:match("^%s*([^#%[%s]%S*)%s*$")
    if value then found[#found + 1] = value end
  end
  return found[1] or "axon", found[2] or "0.1.0"
end

local NAME, VERSION = project()
local PREFIX = os.getenv("PREFIX") or (os.getenv("HOME") .. "/.local")

------------------------------------------------------------------ what was built

local function dim(text)
  return oslo.ui.style(text, { dim = true })
end

local function line(label, value)
  print(dim(oslo.ui.pad(label, 8)) .. value)
end

-- `1524720` -> `1,524,720`. A number this long is read in groups or not at all.
local function grouped(n)
  local text = tostring(math.floor(n))
  local out = text:sub(-3)
  local at = #text - 3
  while at > 0 do
    out = text:sub(math.max(1, at - 2), at) .. "," .. out
    at = at - 3
  end
  return out
end

-- Asked of the ELF, not assumed. `ldd` is not enough on its own: it prints "statically linked" for
-- a binary that still carries an INTERP and will not start.
local function linkage(path)
  local segments = oslo.run{ "readelf", "-l", path, capture = true }
  if not segments.ok then return nil end
  local dynamic = oslo.run{ "readelf", "-d", path, capture = true }
  if (segments.out or ""):find("program interpreter") or (dynamic.out or ""):find("NEEDED") then
    return "dynamic"
  end
  return "static"
end

-- What was built, how big it is, and whether it needs anything on the target machine. Silent when
-- the artifact is not there, so a recipe that builds nothing does not pretend it did.
local function report(path)
  local stat = oslo.fs.stat(path)
  if not stat then return end
  local megabytes = ("%.2f MB"):format(stat.size / 1048576)

  print("")
  print(oslo.ui.title(("%s %s   %s"):format(NAME, VERSION, megabytes)))
  line("binary", path)
  -- Bytes beside megabytes: `1.45 MB` cannot be subtracted from last week's `1.42 MB` to get one.
  line("size", megabytes .. dim("   " .. grouped(stat.size) .. " bytes"))

  local kind = linkage(path)
  if kind == "static" then
    line("linking", oslo.ui.style("✓ static", { fg = "green" }) ..
                    dim("   no runtime dependencies"))
  elseif kind == "dynamic" then
    line("linking", oslo.ui.style("dynamic", { fg = "yellow" }) ..
                    dim("   needs a matching libc on the target machine"))
  end
  print("")
end

-- The same, for artifacts whose exact path the build system decides. Walked with find rather than
-- globbed: oslo's `**` matches a single directory level, and build trees nest deeper than that.
local function report_found(root, pattern)
  local found = oslo.run{ "find", root, "-type", "f", "-name", pattern, capture = true }
  for path in (found.out or ""):gmatch("[^\n]+") do
    report(path)
    return
  end
end


make.recipe{ name = "version", desc = "what this checkout calls itself",
             run = function() print(("%s v%s"):format(NAME, VERSION)) end }

local function need(tool, why)
  assert(oslo.run{ "sh", "-c", "command -v " .. tool, capture = true }.ok, why)
end



make.recipe{
  name = "release",
  desc = "cut a version: --type patch | minor | major | M.m.p",
  params = { { "--type", desc = "patch | minor | major | M.m.p" } },
  run = function(a)
    need("git-rel", "git-rel is not installed; install it first")
    assert(type(a.type) == "string",
           "which release? make release --type patch|minor|major|M.m.p")
    sh.git("rel", a.type)
  end,
}

make.recipe{
  name = "changelog",
  desc = "regenerate CHANGELOG.md",
  run = function()
    need("git-cliff", "git-cliff is not installed; install it first")
    sh.git("cliff", "-o", "CHANGELOG.md")
  end,
}

---------------------------------------------------------------------------- rust

-- The workspace ships one binary, `axon`, and every crate is a library behind it. Recipes name
-- the binary rather than the library: `--lib` builds artifacts nobody runs.
local BIN = "axon"
local RECORDING = os.getenv("RECORDING") or "crates/axon-cli/tests/fixtures/hello.jsonl"

-- Where a demo host listens. Under `$XDG_RUNTIME_DIR` because a Unix socket path must stay
-- shorter than SUN_LEN, and a scratch directory path does not.
local function demo_socket()
  local dir = os.getenv("XDG_RUNTIME_DIR") or "/tmp"
  return dir .. "/axon-demo.sock"
end

local function target_path(release)
  return ("target/%s/%s"):format(release and "release" or "debug", BIN)
end

-- The one target that produces something worth shipping. musl rather than glibc because a
-- glibc "static" build still carries an INTERP and dies on a machine whose loader disagrees;
-- `report` reads the ELF rather than trusting the flag.
local MUSL_TARGET = "x86_64-unknown-linux-musl"

local function dist_path()
  return ("target/%s/release/%s"):format(MUSL_TARGET, BIN)
end

-- Whether this toolchain can actually build for musl.
--
-- `rustc --print target-libdir` answers for a target it has never heard of, so the directory
-- has to be looked at. nixpkgs' plain `rustc` ships one target and no more; a rustup toolchain
-- has whatever was added to it. Asking avoids handing the user forty screens of "can't find
-- crate for `core`" when the honest answer is one line.
local function has_musl_std()
  local dir = oslo.run{ "rustc", "--print", "target-libdir",
                        "--target", MUSL_TARGET, capture = true }
  if not dir.ok then return false end
  local path = (dir.out or ""):match("^%s*(.-)%s*$")
  if path == "" then return false end
  return oslo.run{ "sh", "-c", ("ls %q/libcore-*.rlib >/dev/null 2>&1"):format(path) }.ok
end

-- Where `build` leaves its artifact. Every recipe that needs a binary asks for this one, so
-- `make build` followed by `make run` is a cache hit rather than a second full compile against
-- a different profile.
local function binary_path()
  if has_musl_std() then return dist_path() end
  return ("target/release/%s"):format(BIN)
end

-- Say when what is on PATH is older than what was just built.
--
-- The recurring trap, and it has cost two evenings. `make build` writes into `target/`, `axon`
-- runs whatever is on PATH, and nothing connected the two: you fix a bug, rebuild, run `axon`,
-- and watch the bug you just fixed happen again. Both `build` and `configs` say so now, because
-- each is a moment somebody is about to go and try the thing.
--
-- Below `binary_path` on purpose: it needs `BIN` and `binary_path`, and a `local` declared later
-- in the file is a different variable from the global this would otherwise read.
-- Set while `install` is running, because every warning below is "run `make install`" and
-- printing that to somebody who is running it is noise that reads as a failure.
local installing = false

local function warn_if_install_is_behind()
  if installing then return end
  local installed = PREFIX .. "/bin/" .. BIN
  if not oslo.fs.stat(installed) then
    print(dim("   nothing is installed yet — run `make install`"))
    return
  end
  local newer = oslo.run{ "sh", "-c", ("test %q -nt %q"):format(binary_path(), installed) }
  if newer.ok then
    print(oslo.ui.style("!  ", { fg = "yellow" }) ..
          ("%s is older than what you have built — run `make install`"):format(installed))
  end
end

-- Build it, quietly enough to be a dependency of the recipes that just want to run it.
local function build_binary(announce)
  if not has_musl_std() then
    if announce then
      print(oslo.ui.style("musl target not installed; building against glibc instead",
                          { fg = "yellow" }))
      print(dim("   for a static binary: rustup target add " .. MUSL_TARGET))
    end
    sh.cargo("build", "--release")
    return
  end

  local args = { "cargo", "build", "--release", "--target", MUSL_TARGET }
  -- The flake hands musl over as a path, not a package, so its headers stay off the default
  -- search path and an ordinary build cannot pick them up by accident. Only this recipe is
  -- given it, and only if a C dependency ever needs it -- today nothing here compiles C.
  local musl_cc = os.getenv("MUSL_CC")
  if musl_cc then
    table.insert(args, 1, ("CC_x86_64_unknown_linux_musl=%s/bin/cc"):format(musl_cc))
    table.insert(args, 1, "env")
  end
  assert(oslo.run(args).ok, "the static build failed")
end

make.recipe{
  name = "build",
  desc = "the binary: release, static where the toolchain allows it",
  run = function()
    build_binary(true)
    report(binary_path())
    warn_if_install_is_behind()
  end,
}
make.alias("b", "build")

make.recipe{
  name = "debug",
  desc = "an unoptimized build, for iterating",
  run = function()
    sh.cargo("build")
    report(target_path(false))
  end,
}

make.recipe{
  name = "demo",
  desc = "the UI against a canned recording — no model, no daemon, no tools",
  params = {
    { "--recording", desc = "JSONL session to replay", default = RECORDING },
    { "--pace", desc = "milliseconds between events", default = "40" },
  },
  run = function(a)
    build_binary(false)
    -- Two processes, as the architecture intends: the UI is a socket peer even in a demo.
    -- The host is killed on exit so a second `make run` is not refused a stale socket.
    local script = ([[
      set -e
      socket=%s
      rm -f "$socket"
      %s --socket "$socket" fake-host --replay %s --pace-ms %s >/dev/null 2>&1 &
      host=$!
      trap 'kill $host 2>/dev/null || true; rm -f "$socket"' EXIT INT TERM
      until [ -S "$socket" ]; do sleep 0.05; done
      %s --socket "$socket"
    ]]):format(demo_socket(), binary_path(), a.recording or RECORDING,
               a.pace or "40", binary_path())
    assert(oslo.run{ "sh", "-c", script }.ok, "the UI exited with an error")
  end,
}
make.alias("r", "run")

-- What somebody means by "run it". The daemon is not started here: axon starts its own, for
-- this directory, and stops being our business the moment it exists.
--
-- `demo` used to be called this, which cost an evening: it replays a recording, so the model
-- is a fiction, `/model` has nothing to offer and no prompt reaches anything. A name that
-- promises the product and delivers a fixture is worse than no recipe at all.
make.recipe{
  name = "run",
  desc = "axon, for real, in the current directory",
  params = {
    { "--prompt", desc = "submit this on start, as `axon \"...\"` does" },
  },
  run = function(a)
    build_binary(false)
    local prompt = a.prompt and (" " .. string.format("%q", a.prompt)) or ""
    sh.sh("-c", ("%s%s"):format(binary_path(), prompt))
  end,
}

make.recipe{
  name = "ui",
  desc = "the UI alone, against an already-running host",
  params = {
    { "--socket", desc = "socket to attach to", default = demo_socket() },
  },
  run = function(a)
    build_binary(false)
    sh.sh("-c", ("%s --socket %s"):format(binary_path(), a.socket or demo_socket()))
  end,
}

make.recipe{
  name = "host",
  desc = "a replay host alone, for a UI to attach to",
  params = {
    { "--recording", desc = "JSONL session to replay", default = RECORDING },
    { "--socket", desc = "socket to bind", default = demo_socket() },
  },
  run = function(a)
    build_binary(false)
    local socket = a.socket or demo_socket()
    oslo.run{ "rm", "-f", socket }
    sh.sh("-c", ("%s --socket %s fake-host --replay %s"):format(
      binary_path(), socket, a.recording or RECORDING))
  end,
}

make.recipe{
  name = "install",
  desc = ("install the static binary to %s/bin, and config/ where it reads it"):format(PREFIX),
  -- Built here rather than through `deps`, so the flag is set before anything can warn. A
  -- dependency runs first, and what it printed was "run `make install`" to somebody already
  -- running it -- which reads as a failure at the top of a run that then succeeds.
  run = function()
    installing = true
    build_binary(true)
    report(binary_path())
    local bin = PREFIX .. "/bin"
    assert(oslo.run{ "mkdir", "-p", bin }.ok, "could not create " .. bin)
    assert(oslo.run{ "install", "-m", "755", binary_path(), bin .. "/" .. BIN }.ok,
           "could not install to " .. bin)
    print(("installed %s"):format(bin .. "/" .. BIN))
    -- Last, and part of the install rather than a step to remember: a binary newer than
    -- the config it reads is how a setting that shipped together with it silently does
    -- nothing. Run alone, `configs` still installs only the config.
    make.run("configs")
  end,
}


---------------------------------------------------------------- configuration

-- axon's own configuration lives in `config/`, and this installs it: `config/*` becomes
-- `~/.config/axon/*`. The binary carries a copy of the same files, so a fresh install already
-- speaks and already has a catalog; this is how you get the real ones to edit.
--
-- The same shape as hexe's and oslo's `configs` recipe, deliberately: three tools that install
-- their configuration three different ways is three things to remember.
make.recipe{
  name = "configs",
  desc = "install config/ into $XDG_CONFIG_HOME/axon",
  params = {
    { "--dest", desc = "somewhere other than the config directory" },
    { "--keep", flag = true, desc = "leave installed files you have edited alone" },
  },
  run = function(a)
    assert(oslo.run{ "sh", "-c", "command -v rsync", capture = true }.ok,
           "rsync is not installed; install it first")
    -- Asked of git rather than assumed from the working directory, so this works from anywhere
    -- in the tree. Outside a repository, where the command was run is the best answer there is.
    local top = oslo.run{ "git", "rev-parse", "--show-toplevel", capture = true }
    local root = top.ok and (top.out or ""):match("^%s*(.-)%s*$") or ""
    if root == "" then root = oslo.sys.pwd() end
    local source = root .. "/config"
    assert(oslo.fs.stat(source .. "/"), "there is no config/ directory in " .. root)

    -- The agent layer ships its own client library, the way hexe and oslo ship theirs, and
    -- `config/clients/` is where axon keeps the copies it loads. Copied here rather than
    -- checked in twice: two copies in one repository disagree the first time one is edited, and
    -- the one that would go stale is the one nobody opens.
    --
    -- When that crate leaves the workspace this line becomes what it already is for the other
    -- siblings -- a file that arrives from somewhere else -- and the only change is where it is
    -- copied from.
    local ships = root .. "/crates/axon-agent/lua/agent.lua"
    if oslo.fs.stat(ships) then
      sh.mkdir("-p", source .. "/clients")
      sh.rsync("-a", ships, source .. "/clients/agent.lua")
    end

    local dest = a.dest
    if not dest then
      local config = os.getenv("XDG_CONFIG_HOME")
      if not config or config == "" then config = os.getenv("HOME") .. "/.config" end
      dest = config .. "/" .. NAME
    end
    sh.mkdir("-p", dest)

    -- The repo's `config/` is the source of truth, and installing copies it. That is the whole
    -- job, and for a long time it was not what happened: anything that differed was left alone
    -- and named, so one edited file froze forever and every later fix to it silently never
    -- arrived. Three bugs were diagnosed twice because of it.
    --
    -- `--keep` is the old refusal, for editing the installed copy on purpose. There was a `.bak`
    -- beside anything overwritten for a while, and it went: a directory that grows a second copy
    -- of every file you edit is litter, and the version worth recovering is in git anyway.
    --
    -- Directories are walked file by file, and without `--delete`: a tool you wrote into
    -- `tools/` is not litter.
    local function same(a, b)
      return oslo.run{ "cmp", "-s", a, b }.ok
    end

    local synced, kept = 0, {}
    local function install_file(src, dir, name)
      local dst = dir .. "/" .. name
      if a.keep and oslo.fs.stat(dst) and not same(src, dst) then
        kept[#kept + 1] = dst
        return
      end
      sh.mkdir("-p", dir)
      sh.rsync("-a", src, dst)
      synced = synced + 1
    end


    local function install_tree(src, dst)
      for _, path in ipairs(oslo.fs.glob(src .. "/*")) do
        local name = oslo.path.name(path)
        if oslo.fs.stat(path .. "/") then
          install_tree(path, dst .. "/" .. name)
        else
          install_file(path, dst, name)
        end
      end
    end

    for _, path in ipairs(oslo.fs.glob(source .. "/*")) do
      local name = oslo.path.name(path)
      if oslo.fs.stat(path .. "/") then
        install_tree(path, dest .. "/" .. name)
      else
        install_file(path, dest, name)
      end
    end
    print(oslo.ui.style("✓ ", { fg = "green" }) ..
          ("%d file%s -> %s"):format(synced, synced == 1 and "" or "s", dest))
    if #kept > 0 then
      print(oslo.ui.style("!  ", { fg = "yellow" }) ..
            ("%d file%s left alone because you asked with --keep:")
              :format(#kept, #kept == 1 and "" or "s"))
      for _, path in ipairs(kept) do print(dim("   " .. path)) end
    end

    -- Installed, then read back. axon loads its own configuration, so a file that will not run
    -- is worth knowing about now rather than the next time a daemon starts and quietly falls
    -- back to what it was compiled with.
    --
    -- Read back from the config directory rather than from wherever this was run. `.axon.lua` is
    -- looked for in the working directory, so checking from inside a checkout also loaded that
    -- checkout's project file and printed its refusals: three warnings about this repository, on
    -- every install, saying nothing about what was installed.
    local binary = binary_path()
    if oslo.fs.stat(binary) then
      local home = dest:gsub("/" .. NAME .. "$", "")
      local absolute = binary:sub(1, 1) == "/" and binary or (root .. "/" .. binary)
      local checked = oslo.run{ "sh", "-c",
        ("cd %q && XDG_CONFIG_HOME=%q %q models --all >/dev/null"):format(dest, home, absolute) }
      assert(checked.ok, "the installed configuration does not load")
      print(dim("   it loads"))
    end

    -- Configuration on its own does nothing for a stale binary, and the two are installed by
    -- separate commands.
    warn_if_install_is_behind()
  end,
}

make.recipe{ name = "test", desc = "the suite",
             run = function() sh.cargo("test", "--all-targets", "--release") end }
make.alias("t", "test")

make.recipe{ name = "check", desc = "type-check every target",
             run = function() sh.cargo("check", "--all-targets", "--release") end }

make.recipe{ name = "clippy", desc = "clippy, with warnings denied",
             run = function()
               sh.cargo("clippy", "--all-targets", "--release", "--", "-Dwarnings")
             end }

make.recipe{
  name = "rustdoc",
  desc = "build the docs, with warnings denied",
  run = function()
    local built = oslo.run{
      "env", "RUSTDOCFLAGS=-Dwarnings", "cargo", "doc", "--no-deps", "--release",
    }
    assert(built.ok, "rustdoc failed")
  end,
}

make.recipe{ name = "fmt", desc = "format the workspace",
             run = function() sh.cargo("fmt", "--all") end }

make.recipe{ name = "fmt-check", desc = "fail if anything is unformatted",
             run = function() sh.cargo("fmt", "--all", "--", "--check") end }

-- The architectural gates from PLAN.md §6. Not advisory: each one exists because Pi or Tau
-- shipped the thing it forbids, one reasonable commit at a time.
make.recipe{
  name = "gates",
  desc = "the architectural gates",
  run = function()
    local names = { "gate-file-size", "gate-modules", "gate-proto-size", "gate-reachable" }
    local failed = {}
    for _, name in ipairs(names) do
      local result = oslo.run{ "sh", "scripts/" .. name .. ".sh", capture = true }
      local mark = result.ok and oslo.ui.style("✓", { fg = "green" })
                             or oslo.ui.style("✗", { fg = "red" })
      print(("%s  %s"):format(mark, name))
      if not result.ok then
        failed[#failed + 1] = name
        print(dim("   " .. ((result.out or "") .. (result.err or "")):gsub("\n", "\n   ")))
      end
    end
    assert(#failed == 0, ("%d gate(s) failed"):format(#failed))
  end,
}
make.alias("g", "gates")

make.recipe{ name = "clean", desc = "remove every build output",
             run = function() sh.cargo("clean") end }

make.recipe{ name = "compile", desc = "clean, then build", deps = { "clean", "build" } }
make.alias("c", "compile")

make.recipe{
  name = "verify",
  desc = "the whole local gate",
  deps = { "fmt-check", "check", "test", "clippy", "gates", "rustdoc" },
}
make.alias("v", "verify")
