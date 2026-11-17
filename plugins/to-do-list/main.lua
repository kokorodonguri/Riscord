local S = settings({
  { key = "hide_done", kind = "switch", default = false, label = { ja = "完了したものを隠す", en = "Hide completed tasks", zh = "隐藏已完成的任务" } },
})
local state = S.state
state.todos = state.todos or {}
local function add(text)
  text = trim(text or "")
  if text == "" then return end
  local prio = 0
  text = text:gsub("^!+", function(b) prio = #b return "" end)
  table.insert(state.todos, { id = tostring(riscord.now()) .. #state.todos, text = trim(text), done = false, prio = prio })
  table.sort(state.todos, function(a, b) return (a.prio or 0) > (b.prio or 0) end)
  riscord.save(state)
end
S.extra_page = function()
  local items = {}
  local open = 0
  for _, todo in ipairs(state.todos) do
    if not todo.done then open = open + 1 end
    if not (todo.done and state.hide_done) then
      local mark = string.rep("!", todo.prio or 0)
      items[#items + 1] = { kind = "switch", action = "done:" .. todo.id, label = mark .. (mark ~= "" and " " or "") .. todo.text, enabled = todo.done }
      items[#items + 1] = { kind = "button", action = "del:" .. todo.id, label = t({ ja = "↑ を削除", en = "Delete ↑", zh = "删除 ↑" }) }
    end
  end
  table.insert(items, 1, { kind = "text", label = t({ ja = "残り " .. open .. " 件（先頭に ! を付けると優先度が上がります）", en = open .. " left (start with ! for higher priority)", zh = "剩余 " .. open .. " 项（开头加 ! 可提高优先级）" }) })
  items[#items + 1] = { kind = "input", action = "new", label = t({ ja = "新しいやること", en = "New task", zh = "新任务" }) }
  items[#items + 1] = { kind = "button", action = "add", label = t({ ja = "追加", en = "Add", zh = "添加" }) }
  items[#items + 1] = { kind = "button", action = "clear_done", label = t({ ja = "完了したものを消す", en = "Remove completed", zh = "删除已完成" }) }
  return items
end
S.handle = function(action, values)
  local kind, id = action:match("^(%a+):(.+)$")
  if kind then
    for i, todo in ipairs(state.todos) do
      if todo.id == id then
        if kind == "done" then todo.done = not todo.done else table.remove(state.todos, i) end
        break
      end
    end
  elseif action == "add" then
    add(values.new)
  elseif action == "clear_done" then
    local keep = {}
    for _, todo in ipairs(state.todos) do if not todo.done then keep[#keep + 1] = todo end end
    state.todos = keep
  end
  riscord.save(state)
end
return plugin(S, {
  commands = {
    todo = function(args) add(args) return true end,
  },
})
