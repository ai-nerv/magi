-- A watcher that writes a status line to a file.
--
-- Copy this into `~/.config/magi/plugin/`, then point a status bar at
-- `~/.local/state/magi/status` and it updates as the session runs. Nothing polls; the file is
-- written when something happens.
--
-- **A watcher is told after the fact and answers with nothing.** It cannot change a result and it
-- cannot fail one: a watcher that raised would turn observing a session into a way of breaking
-- it, so a failure here costs this watcher that one observation and nothing else.
--
-- Every event carries `kind`, which is what to branch on. The ones a status line wants:
--
--   session.opened       id, resumed
--   turn.began           model
--   turn.ended           model, took_ms, ok
--   tool.finished        tool, arguments, is_error
--   permission.asked     verb, about
--   permission.answered  verb, about, allowed
--   context.compacted    dropped, kept
--   provider.retried     mind, attempt, of, delay_ms
--
-- Branch on `kind` and nothing else. A watcher that assumed every event had a `tool` field was
-- fine when there was one kind of event and is wrong now.

local WHERE = (os.getenv("XDG_STATE_HOME") or (os.getenv("HOME") .. "/.local/state"))
  .. "/magi/status"

local state = { turns = 0, tools = 0, denied = 0, last = "idle" }

local function write()
  local line = string.format(
    "%s | %d turns | %d tools | %d denied",
    state.last, state.turns, state.tools, state.denied
  )
  -- Gated the same way the write tool is: the person is asked once about this path and not
  -- again. Two returns rather than a raise, so a refusal is something to ignore rather than
  -- something that breaks the turn -- the session is the point, this is decoration.
  magi.fs.write(WHERE, line .. "\n")
end

magi.watch("status-line", {
  run = function(event)
    if event.kind == "session.opened" then
      state.last = event.resumed and "resumed" or "started"
    elseif event.kind == "turn.began" then
      state.turns = state.turns + 1
      state.last = "thinking (" .. tostring(event.model) .. ")"
    elseif event.kind == "turn.ended" then
      state.last = string.format("idle (%dms)", event.took_ms or 0)
    elseif event.kind == "tool.finished" then
      state.tools = state.tools + 1
      state.last = "ran " .. tostring(event.tool)
    elseif event.kind == "permission.answered" then
      if not event.allowed then state.denied = state.denied + 1 end
    elseif event.kind == "provider.retried" then
      state.last = string.format("retrying %d/%d", event.attempt or 0, event.of or 0)
    elseif event.kind == "context.compacted" then
      state.last = string.format("compacted %d entries", event.dropped or 0)
    else
      return
    end
    write()
  end,
})
