return function(ctx)
  local session, completed = ctx.session, ctx.completed
  ctx.submit("editor-running-tool")
  local manager = session.permission_manager
  ctx.wait_for("shell permission", function() return manager:has_pending() end)
  local id = assert(manager.focused_id)
  local pending = assert(manager.pending[id])
  assert(vim.tbl_count(manager.pending) == 1 and #completed == 0)
  local selected
  for index, option in ipairs(pending.sorted_options) do
    if option.kind == "allow_once" then selected = index end
  end
  assert(selected, "native allow once offered")
  vim.api.nvim_set_current_win(session.widget.win_nrs.chat)
  manager:_jump_cursor_to(id)
  ctx.screen("tool-permission-screen.txt", "Allow")
  vim.fn.writefile({ id }, "../tool-permission-ready")
  ctx.wait_for("native permission check", function() return vim.fn.filereadable("../tool-permission-go") == 1 end)
  vim.cmd("stopinsert")
  for index = 2, selected do
    vim.api.nvim_feedkeys("j", "xt", false)
    ctx.wait_for("permission button " .. index, function()
      return session.message_writer:get_focused_button_index(id) == index
    end)
  end
  vim.api.nvim_feedkeys(vim.api.nvim_replace_termcodes("<CR>", true, false, true), "xt", false)
  ctx.wait_for("native running tool observed", function() return vim.fn.filereadable("../tool-cancel-go") == 1 end)
  assert(session.is_generating and #completed == 0 and not manager:has_pending())
  ctx.agentic.stop_generation()
  ctx.wait_for("running tool cancellation", function() return #completed == 1 end)
  assert(completed[1].success, vim.inspect(completed[1].error))
  assert(not session.is_generating and not manager:has_pending())
  assert(ctx.chat():find("was cancelled", 1, true), "native tool cancellation result visible")
  ctx.screen("tool-cancelled-screen.txt", "Generation stopped")
  vim.fn.writefile({ "cancelled" }, "../tool-cancelled")
  ctx.wait_for("native reader reaped", function() return vim.fn.filereadable("../tool-recovery-go") == 1 end)
  ctx.submit("editor-tool-recovery")
  ctx.wait_for("recovery completion", function() return #completed == 2 end)
  assert(completed[2].success, vim.inspect(completed[2].error))
  assert(not session.is_generating and not manager:has_pending())
  ctx.screen("tool-recovered-screen.txt", "editor-tool-recovered")
  vim.fn.writefile({ "recovered" }, "../tool-recovered")
  ctx.wait_for("native recovery checked", function() return vim.fn.filereadable("../tool-recovery-checked") == 1 end)
  return { running_tool_cancelled = true, post_cancel_recovery = true }
end
