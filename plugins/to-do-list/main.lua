local S = settings({
  { key = "hide_done", kind = "switch", default = false, label = { ja = "完了したものを隠す", en = "Hide completed tasks", zh = "隐藏已完成的任务" } },
})
local state = S.state
state.todos = state.todos or {}
local page = 1
local PAGE_SIZE = 40
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
  local visible = {}
  local open = 0
  for _, todo in ipairs(state.todos) do
    if not todo.done then open = open + 1 end
    if not (todo.done and state.hide_done) then
      visible[#visible + 1] = todo
    end
  end
  local pages = math.max(1, math.ceil(#visible / PAGE_SIZE))
  page = math.max(1, math.min(page, pages))
  items[#items + 1] = { kind = "text", label = t({ ja = "残り " .. open .. " 件", en = open .. " left", zh = "剩余 " .. open .. " 项" }) .. " (" .. page .. "/" .. pages .. ")" }
  items[#items + 1] = { kind = "input", action = "new", label = t({ ja = "新しいやること", en = "New task", zh = "新任务" }) }
  items[#items + 1] = { kind = "button", action = "add", label = t({ ja = "追加", en = "Add", zh = "添加" }) }
  items[#items + 1] = { kind = "button", action = "clear_done", label = t({ ja = "完了したものを消す", en = "Remove completed", zh = "删除已完成" }) }
  if page > 1 then items[#items + 1] = { kind = "button", action = "prev_page", label = t({ ja = "前へ", en = "Previous", zh = "上一页" }) } end
  if page < pages then items[#items + 1] = { kind = "button", action = "next_page", label = t({ ja = "次へ", en = "Next", zh = "下一页" }) } end
  for i = (page - 1) * PAGE_SIZE + 1, math.min(page * PAGE_SIZE, #visible) do
    local todo = visible[i]
    local mark = string.rep("!", todo.prio or 0)
    items[#items + 1] = { kind = "switch", action = "done:" .. todo.id, label = mark .. (mark ~= "" and " " or "") .. todo.text, enabled = todo.done }
    items[#items + 1] = { kind = "button", action = "del:" .. todo.id, label = t({ ja = "↑ を削除", en = "Delete ↑", zh = "删除 ↑" }) }
  end
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
  elseif action == "prev_page" then
    page = math.max(1, page - 1)
  elseif action == "next_page" then
    page = page + 1
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
