local S = settings({})
S.state.log = S.state.log or {}
local page = 1
local PAGE_SIZE = 50
-- 部品の数には上限があるので、ボタンを先に置き、記録はページに分ける
S.extra_page = function()
  local items = {}
  local n = #S.state.log
  if n == 0 then
    items[1] = { kind = "text", label = t({ ja = "記録はありません。", en = "Nothing recorded.", zh = "暂无记录。" }) }
    return items
  end
  local pages = math.max(1, math.ceil(n / PAGE_SIZE))
  page = math.max(1, math.min(page, pages))
  items[#items + 1] = { kind = "button", action = "clear", label = t({ ja = "記録を消す", en = "Clear log", zh = "清除记录" }) }
  items[#items + 1] = { kind = "text", label = n .. t({ ja = " 件", en = " entries", zh = " 条" }) .. " (" .. page .. "/" .. pages .. ")" }
  if page > 1 then items[#items + 1] = { kind = "button", action = "prev_page", label = t({ ja = "前へ", en = "Previous", zh = "上一页" }) } end
  if page < pages then items[#items + 1] = { kind = "button", action = "next_page", label = t({ ja = "次へ", en = "Next", zh = "下一页" }) } end
  -- 新しいものから
  local first = n - (page - 1) * PAGE_SIZE
  for i = first, math.max(1, first - PAGE_SIZE + 1), -1 do items[#items + 1] = { kind = "text", label = S.state.log[i] } end
  return items
end
S.handle = function(action)
  if action == "clear" then
    S.state.log = {}
    page = 1
    riscord.save(S.state)
  elseif action == "prev_page" then
    page = math.max(1, page - 1)
  elseif action == "next_page" then
    page = page + 1
  end
end
return plugin(S, {
  on_removed = function(kind, id, label)
    local msg = kind == "friend"
      and t({ ja = "フレンドではなくなりました: ", en = "No longer friends: ", zh = "已不再是好友：" }) .. label
      or t({ ja = "サーバーから外れました: ", en = "Removed from server: ", zh = "已离开服务器：" }) .. label
    table.insert(S.state.log, riscord.date("%Y-%m-%d %H:%M") .. "  " .. msg)
    while #S.state.log > 200 do table.remove(S.state.log, 1) end
    riscord.save(S.state)
    riscord.notify("RemovedConnectionAlerts", msg)
  end,
})
