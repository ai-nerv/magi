-- Inline `/goal` command and autonomous goal loop plugin.
--
-- Drop into `~/.config/magi/plugin/goal.lua` to activate. Works anywhere in a prompt:
--   "refactor the parser and verify tests /goal"
--   "/goal fix the flaky tests in magi-host"
--   "/goal stop"

local state = {
  active = false,
  goal = nil,
  step = 0,
  max = 12,
}

magi.command("goal", {
  description = "Keep working autonomously until the goal is complete (/goal stop to halt)",
  run = function(ctx)
    local arg = (ctx.args or ""):match("^%s*(.-)%s*$")
    if arg == "stop" or arg == "off" then
      state.active = false
      state.goal = nil
      state.step = 0
      return { handled = true, notice = "Goal loop stopped.", status = false }
    end
    local task = (ctx.cleaned or ""):match("^%s*(.-)%s*$")
    if task == "" then
      local msg = state.active
        and ("Goal active (step " .. state.step .. "/" .. state.max .. "): " .. (state.goal or ""))
        or "Usage: include /goal anywhere in a prompt, or `/goal stop` to halt."
      return { handled = true, notice = msg }
    end
    state.active = true
    state.goal = task
    state.step = 0
    return {
      prompt = task,
      aside = "Autonomous goal mode is active. Work thoroughly step by step, verify builds and tests, and ONLY when the entire goal is achieved, include the exact line `GOAL_COMPLETE` at the end of your final message.",
      notice = "Goal loop started (max " .. state.max .. " turns; press Esc or type `/goal stop` to halt).",
      status = "goal 1/" .. state.max,
    }
  end,
})

magi.hook("goal-loop", {
  before_turn = function(_ctx)
    if not state.active then
      return nil
    end
    return {
      system = "# Active Goal\n"
        .. (state.goal or "")
        .. "\nKeep working until completely finished and verified, then include `GOAL_COMPLETE`.",
      status = "goal " .. (state.step + 1) .. "/" .. state.max,
    }
  end,
  after_turn = function(ctx)
    if not state.active then
      return nil
    end
    if ctx.cancelled or not ctx.ok then
      state.active = false
      state.goal = nil
      return { notice = "Goal loop halted.", status = false }
    end
    if (ctx.text or ""):find("GOAL_COMPLETE", 1, true) then
      local turns = state.step + 1
      state.active = false
      state.goal = nil
      return { notice = "Goal achieved in " .. turns .. " turn(s).", status = false }
    end
    state.step = state.step + 1
    if state.step >= state.max then
      state.active = false
      state.goal = nil
      return { notice = "Goal loop reached limit (" .. state.max .. " turns).", status = false }
    end
    return {
      continue = "Continue working on the goal. Check what remains, verify your changes, and include `GOAL_COMPLETE` once everything is done.",
      notice = "Goal loop continuing (" .. state.step .. "/" .. state.max .. ")…",
      status = "goal " .. (state.step + 1) .. "/" .. state.max,
    }
  end,
})
