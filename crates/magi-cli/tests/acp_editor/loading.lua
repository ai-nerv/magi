local function wait_for(label, predicate)
  assert(vim.wait(15000, predicate, 10), label .. " deadline")
end

local function run()
  assert(vim.fn.has("nvim-0.11") == 1)
  assert(#vim.api.nvim_list_uis() > 0, "real terminal UI attached")
  vim.o.columns, vim.o.lines = 120, 32
  vim.opt.runtimepath:prepend(assert(vim.env.MAGI_TEST_EDITOR_CLIENT))
  local agentic = require("agentic")
  local completed = {}
  agentic.setup({
    provider = "magi-native",
    acp_providers = {
      ["magi-native"] = { name = "Magi native fixture", command = assert(vim.env.MAGI_TEST_BINARY), args = { "acp" } },
    },
    image_paste = { enabled = false },
    hooks = { on_response_complete = function(data) completed[#completed + 1] = data end },
  })
  local loading = vim.env.MAGI_EDITOR_LOAD == "1"
  local identity
  if loading then
    identity = assert(vim.fn.readfile("../saved-session.txt")[1])
    agentic.restore_session_by_id(identity)
  else
    agentic.open({ auto_add_to_context = false })
  end
  local registry = require("agentic.session_registry")
  wait_for("ready native session", function()
    local session = registry.current()
    return session and session.session_id ~= nil and session:can_submit_prompt()
  end)
  local session = assert(registry.current())
  if loading then assert(session.session_id == identity) end
  assert(session.agent.agent_capabilities.loadSession, "persistent loading advertised")
  assert(session.widget:is_open(), "actual chat window")
  local pid = assert(session.agent.transport.pid)
  local executable = assert(vim.uv.fs_readlink("/proc/" .. pid .. "/exe"))
  assert(executable == vim.env.MAGI_TEST_BINARY, "selected running adapter")
  local function chat()
    return table.concat(vim.api.nvim_buf_get_lines(session.widget.buf_nrs.chat, 0, -1, false), "\n")
  end
  local function screen(name, expected)
    vim.cmd("redraw!")
    local rows = {}
    for row = 1, vim.o.lines do
      local cells = {}
      for col = 1, vim.o.columns do cells[#cells + 1] = vim.fn.screenstring(row, col) end
      rows[#rows + 1] = table.concat(cells)
    end
    vim.fn.writefile(rows, "../" .. name)
    assert(table.concat(rows, "\n"):find(expected, 1, true), "rendered " .. expected)
  end
  if loading then
    assert(#completed == 0 and not session.is_generating, "replay is not a generated turn")
    assert(chat():find("editor-persistent-first", 1, true), "restored user message")
    assert(chat():find("part-0-done", 1, true), "restored agent message")
    screen("load-replay-screen.txt", "part-0-done")
    vim.fn.writefile({ chat() }, "../load-replay-buffer.txt")
    vim.fn.writefile({ "ready" }, "../load-replayed")
    wait_for("provider replay exclusion", function() return vim.fn.filereadable("../load-replay-checked") == 1 end)
  end
  local text = loading and "editor-persistent-next" or "editor-persistent-first"
  local prefix = loading and "part-1" or "part-0"
  vim.api.nvim_buf_set_lines(session.widget.buf_nrs.input, 0, -1, false, { text })
  session.widget:_submit_input()
  wait_for("native prompt prefix", function() return chat():find(prefix, 1, true) ~= nil and session.is_generating end)
  vim.fn.writefile({ "ready" }, loading and "../load-next-ready" or "../load-seed-ready")
  wait_for("completed native turn", function() return #completed == 1 end)
  assert(completed[1].success)
  assert(chat():find(prefix .. "-done", 1, true))
  screen(loading and "load-continued-screen.txt" or "load-seed-screen.txt", prefix .. "-done")
  if not loading then vim.fn.writefile({ session.session_id }, "../saved-session.txt") end
  vim.fn.writefile({ vim.json.encode({
    session = session.session_id, adapter = { pid = pid, executable = executable },
    client_commit = "f0bef086b3ed6121d9a18a5a820d071c69f2f0f5",
    restored = loading, completions = #completed, buffer = chat(), terminal_rendered = true,
  }) }, "../load-editor-receipt.json")
end

vim.schedule(function()
  local ok, why = xpcall(run, debug.traceback)
  if not ok then
    vim.fn.writefile({ tostring(why) }, "../editor-error.txt")
    vim.cmd("cquit 1")
  else
    vim.cmd("qa!")
  end
end)
