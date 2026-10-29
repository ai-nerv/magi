local function wait_for(label, predicate)
  assert(vim.wait(15000, predicate, 10), label .. " deadline")
end

local function run()
  assert(vim.fn.has("nvim-0.11") == 1, "Neovim 0.11 or newer required")
  local terminal = vim.env.MAGI_EDITOR_TERMINAL == "1"
  if terminal then
    assert(#vim.api.nvim_list_uis() > 0, "real terminal UI must be attached")
    vim.o.columns = 120
    vim.o.lines = 32
  end
  local function screen(name, expected, redraw)
    if not terminal then return end
    if redraw ~= false then vim.cmd("redraw!") end
    local rows = {}
    for row = 1, vim.o.lines do
      local cells = {}
      for col = 1, vim.o.columns do cells[#cells + 1] = vim.fn.screenstring(row, col) end
      rows[#rows + 1] = table.concat(cells)
    end
    vim.fn.writefile(rows, "../" .. name)
    assert(table.concat(rows, "\n"):find(expected, 1, true), "rendered screen missing " .. expected)
  end
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
  agentic.open({ auto_add_to_context = false })
  local registry = require("agentic.session_registry")
  wait_for("editor native session", function()
    local session = registry.current()
    return session and session.session_id ~= nil
  end)
  local session = assert(registry.current())
  local pid = assert(session.agent.transport.pid, "running ACP adapter PID")
  local executable = assert(vim.uv.fs_readlink("/proc/" .. pid .. "/exe"), "running ACP executable")
  local adapter = { pid = pid, executable = executable }
  vim.fn.writefile({ vim.json.encode(adapter) }, "../adapter-identity.json")
  assert(executable == vim.env.MAGI_TEST_BINARY, "editor must launch the selected ACP executable")
  assert(session.widget:is_open(), "real editor chat window must exist")
  local function chat()
    return table.concat(vim.api.nvim_buf_get_lines(session.widget.buf_nrs.chat, 0, -1, false), "\n")
  end
  local function submit(text)
    vim.api.nvim_buf_set_lines(session.widget.buf_nrs.input, 0, -1, false, { text })
    session.widget:_submit_input()
  end
  local identity = session.session_id
  local fields = dofile("../scenario.lua")({
    session = session, completed = completed, chat = chat, submit = submit,
    wait_for = wait_for, screen = screen, agentic = agentic,
  })
  assert(registry.current().session_id == identity, "scenario preserves native session")
  local receipt = {
    editor = vim.version(),
    client = "agentic.nvim",
    client_commit = "f0bef086b3ed6121d9a18a5a820d071c69f2f0f5",
    adapter = adapter,
    session = session.session_id,
    completions = #completed,
    buffer = chat(),
    visual_acceptance = false,
    persistent_load_acceptance = false,
    terminal_rendered = terminal,
  }
  for key, value in pairs(fields) do receipt[key] = value end
  vim.fn.writefile({ vim.json.encode(receipt) }, "../editor-receipt.json")
end

local function main()
  local ok, why = xpcall(run, debug.traceback)
  if not ok then
    local session = require("agentic.session_registry").current()
    local failure = {
      messages = vim.api.nvim_exec2("messages", { output = true }).output,
      generating = session and session.is_generating,
      buffer = session and vim.api.nvim_buf_get_lines(session.widget.buf_nrs.chat, 0, -1, false),
    }
    vim.fn.writefile({ vim.json.encode(failure) }, "../editor-failure.json")
    vim.fn.writefile({ tostring(why) }, "../editor-error.txt")
    vim.cmd("cquit 1")
  else
    vim.cmd("qa!")
  end
end
if vim.env.MAGI_EDITOR_TERMINAL == "1" then vim.schedule(main) else main() end
