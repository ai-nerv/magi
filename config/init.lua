-- axon's configuration, and its only entry point.
--
-- A program, not a data file: it may probe the machine, loop, and branch. Settings are
-- assigned, descriptions go to a registrar, and the file returns nothing.

-- What runs. Nothing is discovered by scanning: a file not named here does not load.
-- Clients first — a tool loads its sibling's client library as it declares itself.
axon.load("clients/hexe.lua")
axon.load("clients/oslo.lua")
axon.load("apis.lua")
axon.load("providers.lua")
axon.load("tools.lua")

-- Which model to use, as `axon models` prints it.
axon.model = "anthropic/claude-sonnet-4-5"

-- An endpoint of your own, if it speaks a dialect axon already knows. Registration is keyed,
-- so declaring the same id twice replaces rather than appends.
--
-- axon.provider("my-box", {
--   name = "My GPU box",
--   api = "openai-completions",
--   base_url = "http://10.0.0.7:8000/v1",
--   auth = { kind = "none" },
--   models = {
--     { id = "qwen3-coder", name = "Qwen3 Coder", context_window = 262144, max_tokens = 32768 },
--   },
-- })

-- Whether `read`, `write` and `edit` refuse paths outside the session's directory. Off: it
-- moved work to the shell, which has no confinement at all. `bwrap` in front of the shell peer
-- is what actually contains anything.
--
-- axon.confine = true

-- Permissions granted in advance, so they are not asked about. Each rule is a verb — `read`,
-- `write`, `run`, `reach` — and one width. A rule naming no width grants nothing.
--
--   anything = true        every action of that verb, anywhere
--   directory = "/path"    that path and everything under it
--   program = "git"        any command whose first word is this
--
-- axon.allow = {
--   { verb = "read",  directory = "/home/you/work" },
--   { verb = "run",   program = "git" },
-- }

-- How long a daemon stays up with nobody attached and nothing running, in seconds. Detaching
-- is not ending a session, so this is a grace period rather than a hangup; 0 keeps it forever.
--
-- axon.idle_exit = 600

-- Environment every process axon starts is given, on top of what it inherits. `OSLO_PROFILE`
-- is set to "axon" whether or not this says so, and naming it here overrides that.
--
-- axon.env = { PAGER = "cat", RUST_LOG = "warn" }

-- Directories whose `.axon.lua` is as trusted as this file. A project file may otherwise set
-- settings but not declare a provider, a tool or a peer.
--
-- axon.trusted = { "/home/you/work" }

-- ---------------------------------------------------------------------- the screen
--
-- Everything the UI draws with is a setting under `axon.ui`. Three kinds:
--
-- COLOURS are palette indices, 0-255, and mean whatever your terminal says they mean:
--
--   accent  success  warning  error  typed
--   md_heading  md_code  md_code_block  md_quote
--   diff_added  diff_added_bg  diff_removed  diff_removed_bg  diff_context
--   tool_bg  tool_title  tool_ok  tool_failed  tool_output  tool_fold
--   menu_selected_bg  menu_selected  menu_detail  menu_detail_selected  menu_meta
--   border  scan  hint  rule
--   message_rail  message_bg  message_text
--   thinking  text  muted  dim
--
-- `border` and `scan` are the ends of a gradient: the light walks the indices between them, so
-- keep them far enough apart to have something to walk.
--
-- GLYPHS are any string, and width is your problem — a two-column glyph draws two columns:
--
--   corner_top_left  corner_top_right  corner_bottom_left  corner_bottom_right
--   edge_horizontal  edge_vertical
--   marker  no_marker  ellipsis  bullet
--   more_rule  expand  collapse  quote_rule  notice_rule
--   placeholder_short  no_model  decrypt_pool  flicker_pool  type_stages  heartbeat
--   spinner       a list of frames: { "◐", "◓", "◑", "◒" }
--   placeholders  a list of lines, one shown a session (see below)
--
-- NUMBERS are rows, budgets and rates. Anything named `_share` is a percentage. A value under
-- its floor is raised rather than refused:
--
--   footer_rows  prompt_min_rows
--   live_rows  live_share  prompt_share  prompt_min_lines
--   menu_rows  preview_lines  page_share
--   block_pad  gutter  tab_width  column_gap  min_column
--   summary_budget  argument_floor
--   frame_ms  scan_speed  scan_nose  scan_tail
--   rest_pace  hold_pace  work_pace
--   decrypt_ms  flicker_odds  flicker_ms  type_reveal_ms
--   tease_after_ms  tease_step_ms  tease_doubt_ms
--   beacon_ms  beacon_cells  footer_pad

-- The footer is three columns held clear of both edges: what this session calls itself on the
-- left, the display in the middle, the model on the right. The display is `beacon_cells` wide --
-- and it is drawn as a monitor: the trace scrolls continuously right to left, and what runs
-- through it is the signal the session is putting out. A heartbeat while a turn is running, a flat line when nothing is, a
-- square wave while a permission or a list waits on you, a tighter one while `/` narrows a menu,
-- and a line with the lead off when the daemon is away. Anything open on screen outranks what
-- the agent is doing, because it is the thing holding everything up. One colour, the footer's
-- own. `beacon_ms` is how long the trace takes to scroll one display width, and it is the same
-- for every state: one tape at one speed, so a turn starting or ending scrolls in rather than
-- cutting to a different picture.
--
-- `beacon_cells` is a preference, not a promise: the display is centred on the row, and landing
-- on the exact middle needs it to be the same parity as the terminal is wide, so it is widened
-- by one where it has to be.
--
axon.ui.footer_pad   = 3
axon.ui.beacon_ms    = 1000
axon.ui.beacon_cells = 9

-- The opening scramble: text lands as noise and resolves into itself over `decrypt_ms`
-- milliseconds. Zero, the built-in, is no effect -- set it to switch the thing on. It runs once,
-- over the whole screen, and leaves the box and every other frame character alone.
--
axon.ui.decrypt_ms = 900
axon.ui.decrypt_pool = "0#$%&@?*"

-- And the box never quite settling: one character in `flicker_odds` glitches to a symbol for
-- `flicker_ms` and comes back as itself. Zero odds, the built-in, is off. Only inside the prompt
-- box -- a glitch in a tool result is indistinguishable from a tool that printed one.
--
-- `flicker_odds` is one in N, so a BIGGER number is rarer. 3000 is a character every few
-- seconds; 250 is a fidget.
--
axon.ui.flicker_odds = 800
axon.ui.flicker_ms   = 180

-- What the box says when you sit down. Plain, and read once: a joke in this position is a joke
-- in the way. A fresh one each time the prompt empties.
axon.ui.openers = {
  "let's build something",
  "what are we making?",
  "let's scan the project",
  "where shall we start?",
  "what needs doing?",
  "let's have a look",
  "describe the change",
  "what is broken?",
}

-- What it writes to itself once you have left it alone for `tease_after_ms`.
--
-- `a ~~b~~ c` is performed, not drawn: it writes `a b`, reads it back for a moment, deletes the
-- `b` a character at a time, and writes `c` in its place. Watching it happen is the joke.
axon.ui.placeholders = {
  -- The work
  "first we need to build a ~~tool~~ tool to build the tool",
  "the scaffolding is ~~temporary~~ the building",
  "this is a ~~prototype~~ load-bearing prototype",
  "I'll build it ~~properly~~ twice",
  "first the ~~feature~~ build system",
  "we're ~~building~~ assembling this out of npm",
  "let's build it ~~in-house~~ out of four dependencies",
  "I'm making a ~~library~~ folder called utils",

  -- The ground it stands on
  "we're building on ~~solid ground~~ someone's weekend project",
  "the foundation is ~~poured~~ a TODO from March",
  "let's start ~~fresh~~ from a template nobody read",
  "the architecture is ~~designed~~ whatever compiled first",
  "the build is ~~reproducible~~ on this laptop",

  -- What we said we'd make
  "the spec is ~~written down~~ in someone's head",
  "the roadmap is ~~a plan~~ a list of wishes",
  "we'll build the real one ~~next quarter~~ eventually",
  "let's cut scope to ~~the essentials~~ whatever already works",
  "the demo is ~~working~~ three hardcoded strings",
  "it's ~~done~~ done except the parts that matter",

  -- Making it nice
  "let's make it ~~extensible~~ hard to change in one place",
  "this abstraction ~~saves time~~ needs an abstraction",
  "we're making it ~~simple~~ simple to describe",
  "I'm ~~designing~~ redesigning the schema",
  "let's ~~generalise~~ wait until it happens twice",
  "I'll make it fast ~~now~~ after it works",
  "we're ~~shipping~~ shipping and watching the graphs",
}

-- And what you type arriving: each character shows as the first of `type_stages`, passes through
-- the rest, and lands as the letter, all within `type_reveal_ms`. Zero, the built-in, is off.
--
-- The time is split evenly across the stages, so give it enough that each one gets a frame or
-- more: three stages in 300ms is 100ms each against a default `frame_ms` of 80.
--
axon.ui.type_reveal_ms = 60
axon.ui.type_stages    = "·*#"

-- For a palette generated by lule, where 1-6 are pigments in chroma order rather than hues and
-- 232-255 runs black → colour 0 → accent → colour 15 → white:
--
-- axon.ui = {
--   accent = 1, success = 2, warning = 3, error = 9,
--   md_heading = 5, md_code = 1, md_code_block = 2, md_quote = 250,
--   diff_added = 2, diff_removed = 9, diff_context = 250,
--   typed = 14,
--   dim = 8, muted = 250, text = 252, thinking = 250, rule = 8, hint = 8,
--   tool_title = 254, tool_ok = 2, tool_failed = 9, tool_output = 251, tool_fold = 8,
--   menu_selected = 255, menu_detail = 250, menu_detail_selected = 255, menu_meta = 8,
--   message_text = 254,
--   -- 232-236 is below your background: a surface there is a hole.
--   tool_bg = 239, menu_selected_bg = 242, message_bg = 242,
--   -- Keep the border low: 243-244 is your accent at full strength, and a border there glows
--   -- brightly enough that the light moving along it disappears into its own frame.
--   border = 238, scan = 254,
-- }

-- What axon tells the model it is. axon appends the session's facts (directory, platform,
-- date) and the project's `AGENTS.md`.
axon.system = [[
You are axon, a coding agent working in a terminal alongside a person at their computer.

Do the work rather than describing it. When a change is needed, make it with `edit` or `write`;
when something needs checking, check it with `read` or `shell`. Prefer reading the code to
guessing about it, and prefer running a command to predicting its output.

Match the code you are editing: its naming, its idioms, its comment density. A change that
reads like the file around it is easier to review than one that is merely correct.

Be brief. The person is reading a terminal, not a report. Say what you did and what it means;
skip preamble, skip summarising what they just watched happen, and do not close by offering
further help. If something failed, say so plainly with the output rather than hedging.

Ask only when the answer would change what you do and you cannot find it yourself. Otherwise
make the ordinary judgement call, say which one you made, and carry on.
]]
