return function(ctx)
  local session, completed = ctx.session, ctx.completed
  local pending_ids = {}
  local function permission(stage, kind, number, prompt)
    ctx.submit(prompt)
    local manager = session.permission_manager
    ctx.wait_for(stage .. " permission", function() return manager:has_pending() end)
    assert(session.is_generating and #completed == number - 1)
    local id = assert(manager.focused_id)
    local pending = assert(manager.pending[id])
    assert(vim.tbl_count(manager.pending) == 1)
    assert(not vim.tbl_contains(pending_ids, id), "fresh native approval identifier")
    pending_ids[#pending_ids + 1] = id
    local selected
    for index, option in ipairs(pending.sorted_options) do
      if option.kind == kind then selected = index end
    end
    assert(selected, "offered permission option " .. kind)
    vim.api.nvim_set_current_win(session.widget.win_nrs.chat)
    manager:_jump_cursor_to(id)
    ctx.screen(stage == "allow" and "allow-pending-screen.txt" or stage .. "-screen.txt", kind == "allow_once" and "Allow" or "Refuse")
    vim.fn.writefile({ id }, "../" .. stage .. "-ready")
    ctx.wait_for(stage .. " native check", function() return vim.fn.filereadable("../" .. stage .. "-go") == 1 end)
    vim.cmd("stopinsert")
    for index = 2, selected do
      vim.api.nvim_feedkeys("j", "xt", false)
      ctx.wait_for("permission button " .. index, function()
        return session.message_writer:get_focused_button_index(id) == index
      end)
    end
    vim.api.nvim_feedkeys(vim.api.nvim_replace_termcodes("<CR>", true, false, true), "xt", false)
    ctx.wait_for(stage .. " completion", function() return #completed == number end)
    assert(completed[number].success, vim.inspect(completed[number].error))
    assert(not manager:has_pending())
    assert(not session.is_generating)
  end
  permission("allow", "allow_once", 1, "editor-allow-read-once")
  assert(ctx.chat():find("editor-allowed-read", 1, true), "real read result visible in editor")
  permission("deny-read", "reject_once", 2, "editor-deny-repeated-read")
  assert(not ctx.chat():find("editor-never-read", 1, true))
  permission("deny-shell", "reject_once", 3, "editor-deny-shell-effect")
  assert(vim.fn.filereadable("forbidden") == 0)
  ctx.screen("permissions-completed-screen.txt", "editor-deny-shell-complete")
  vim.fn.writefile({ "permission-scenario-complete" }, "../permissions-complete")
  ctx.wait_for("native permission effects verified", function() return vim.fn.filereadable("../permissions-checked") == 1 end)
  return { allowed_once = 1, rejected_once = 2, permission_ids = pending_ids }
end
