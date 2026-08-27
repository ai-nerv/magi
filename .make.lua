-- axum's build, as recipes. This replaced the Makefile; there is no other.
--
--   make            the recipes, with what each of them says it does
--   make build      the binary
--   make test       the suite
--   make verify     the whole local gate
--
-- At an oslo prompt in this directory `make` is enough; everywhere else it is `oslo make`.
-- CI has no oslo, so it calls the language's own tool -- nothing here is on the release path.

local make = oslo.make

-- Name and version live in PROJECT, one per line, so every tool reads them from one place.
local function project()
  local found = {}
  for line in (oslo.fs.read("PROJECT") or ""):gmatch("[^\n]+") do
    local value = line:match("^%s*([^#%[%s]%S*)%s*$")
    if value then found[#found + 1] = value end
  end
  return found[1] or "axum", found[2] or "0.1.0"
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

-- The workspace ships one binary, `axum`, and every crate is a library behind it. Recipes name
-- the binary rather than the library: `--lib` builds artifacts nobody runs.
local BIN = "axum"
local RECORDING = os.getenv("RECORDING") or "examples/recordings/hello.jsonl"

-- Where a demo host listens. Under `$XDG_RUNTIME_DIR` because a Unix socket path must stay
-- shorter than SUN_LEN, and a scratch directory path does not.
local function demo_socket()
  local dir = os.getenv("XDG_RUNTIME_DIR") or "/tmp"
  return dir .. "/axum-demo.sock"
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

-- Which backend the UI should draw with.
--
-- `--alt` is the default because a buffer we own is the only one a future feature can search,
-- select in, or jump through; `--inline` opts back into letting the terminal keep the history.
-- Passing both is a contradiction rather than a precedence puzzle, so it is refused.
local function tui_mode(a)
  assert(not (a.alt and a.inline), "pass --alt or --inline, not both")
  if a.inline then return "inline" end
  return "alt"
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
    { "--alt", desc = "alt screen; axum owns the buffer and the history", flag = true },
    { "--inline", desc = "inline viewport; the terminal keeps the history", flag = true },
    { "--recording", desc = "JSONL session to replay", default = RECORDING },
    { "--pace", desc = "milliseconds between events", default = "40" },
  },
  run = function(a)
    build_binary(false)
    local mode = tui_mode(a)
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
      %s --socket "$socket" --tui %s
    ]]):format(demo_socket(), binary_path(), a.recording or RECORDING,
               a.pace or "40", binary_path(), mode)
    assert(oslo.run{ "sh", "-c", script }.ok, "the UI exited with an error")
  end,
}
make.alias("r", "run")

-- What somebody means by "run it". The daemon is not started here: axum starts its own, for
-- this directory, and stops being our business the moment it exists.
--
-- `demo` used to be called this, which cost an evening: it replays a recording, so the model
-- is a fiction, `/model` has nothing to offer and no prompt reaches anything. A name that
-- promises the product and delivers a fixture is worse than no recipe at all.
make.recipe{
  name = "run",
  desc = "axum, for real, in the current directory",
  params = {
    { "--alt", desc = "alt screen; axum owns the buffer and the history", flag = true },
    { "--inline", desc = "inline viewport; the terminal keeps the history", flag = true },
    { "--prompt", desc = "submit this on start, as `axum \"...\"` does" },
  },
  run = function(a)
    build_binary(false)
    local prompt = a.prompt and (" " .. string.format("%q", a.prompt)) or ""
    sh.sh("-c", ("%s --tui %s%s"):format(binary_path(), tui_mode(a), prompt))
  end,
}

make.recipe{
  name = "ui",
  desc = "the UI alone, against an already-running host",
  params = {
    { "--alt", desc = "alt screen; axum owns the buffer and the history", flag = true },
    { "--inline", desc = "inline viewport; the terminal keeps the history", flag = true },
    { "--socket", desc = "socket to attach to", default = demo_socket() },
  },
  run = function(a)
    build_binary(false)
    sh.sh("-c", ("%s --socket %s --tui %s"):format(
      binary_path(), a.socket or demo_socket(), tui_mode(a)))
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
  desc = ("install the static binary to %s/bin"):format(PREFIX),
  deps = { "build" },
  run = function()
    local bin = PREFIX .. "/bin"
    assert(oslo.run{ "mkdir", "-p", bin }.ok, "could not create " .. bin)
    assert(oslo.run{ "install", "-m", "755", binary_path(), bin .. "/" .. BIN }.ok,
           "could not install to " .. bin)
    print(("installed %s"):format(bin .. "/" .. BIN))
  end,
}


---------------------------------------------------------------- configuration

-- axum's own configuration lives in `config/`, and this installs it: `config/*` becomes
-- `~/.config/axum/*`. The binary carries a copy of the same files, so a fresh install already
-- speaks and already has a catalog; this is how you get the real ones to edit.
--
-- The same shape as hexe's and oslo's `configs` recipe, deliberately: three tools that install
-- their configuration three different ways is three things to remember.
make.recipe{
  name = "configs",
  desc = "install config/ into $XDG_CONFIG_HOME/axum",
  params = { { "--dest", desc = "somewhere other than the config directory" } },
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

    local dest = a.dest
    if not dest then
      local config = os.getenv("XDG_CONFIG_HOME")
      if not config or config == "" then config = os.getenv("HOME") .. "/.config" end
      dest = config .. "/" .. NAME
    end
    sh.mkdir("-p", dest)

    -- One entry at a time, each mirrored with --delete, rather than one --delete over the whole
    -- tree: the destination is where anything else you keep beside init.lua lives, and a
    -- tree-wide mirror would take it with it.
    local synced = 0
    for _, path in ipairs(oslo.fs.glob(source .. "/*")) do
      local name = oslo.path.name(path)
      if oslo.fs.stat(path .. "/") then
        sh.mkdir("-p", dest .. "/" .. name)
        sh.rsync("-a", "--delete", path .. "/", dest .. "/" .. name .. "/")
      else
        sh.rsync("-a", path, dest .. "/" .. name)
      end
      synced = synced + 1
    end
    print(oslo.ui.style("✓ ", { fg = "green" }) ..
          ("%d entr%s -> %s"):format(synced, synced == 1 and "y" or "ies", dest))

    -- Installed, then read back. axum loads its own configuration, so a file that will not run
    -- is worth knowing about now rather than the next time a daemon starts and quietly falls
    -- back to what it was compiled with.
    local binary = binary_path()
    if oslo.fs.stat(binary) then
      local home = dest:gsub("/" .. NAME .. "$", "")
      local checked = oslo.run{ "sh", "-c",
        ("XDG_CONFIG_HOME=%q %q models --all >/dev/null"):format(home, binary) }
      assert(checked.ok, "the installed configuration does not load")
      print(dim("   it loads"))
    end

    -- Configuration on its own does nothing for a stale binary, and the two are installed by
    -- separate commands. Somebody who ran `make configs` and found nothing had changed spent
    -- an evening on a build from a milestone ago; saying so here costs a line.
    local installed = PREFIX .. "/bin/" .. BIN
    local there = oslo.fs.stat(installed)
    if not there then
      print(dim("   nothing is installed yet — run `make install`"))
    else
      local newer = oslo.run{ "sh", "-c",
        ("test %q -nt %q"):format(binary_path(), installed) }
      if newer.ok then
        print(oslo.ui.style("!  ", { fg = "yellow" }) ..
              ("%s is older than what you have built — run `make install`"):format(installed))
      end
    end
  end,
}

make.recipe{ name = "test", desc = "the suite",
             run = function() sh.cargo("test", "--all-targets") end }
make.alias("t", "test")

make.recipe{ name = "check", desc = "type-check every target",
             run = function() sh.cargo("check", "--all-targets") end }

make.recipe{ name = "clippy", desc = "clippy, with warnings denied",
             run = function()
               sh.cargo("clippy", "--all-targets", "--", "-Dwarnings")
             end }

make.recipe{
  name = "rustdoc",
  desc = "build the docs, with warnings denied",
  run = function()
    local built = oslo.run{ "env", "RUSTDOCFLAGS=-Dwarnings", "cargo", "doc", "--no-deps" }
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
    local names = { "gate-file-size", "gate-proto-size", "gate-reachable" }
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
