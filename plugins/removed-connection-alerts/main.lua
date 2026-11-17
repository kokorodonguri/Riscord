local S = settings({})
S.state.log = S.state.log or {}
S.extra_page = function()
  local items = {}
  if #S.state.log == 0 then items[1] = { kind = "text", label = t({ ja = "記録はありません。", en = "Nothing recorded.", zh = "暂无记录。" }) } end
  for i = #S.state.log, 1, -1 do items[#items + 1] = { kind = "text", label = S.state.log[i] } end
  items[#items + 1] = { kind = "button", action = "clear", label = t({ ja = "記録を消す", en = "Clear log", zh = "清除记录" }) }
  return items
end
S.handle = function(action) if action == "clear" then S.state.log = {}; riscord.save(S.state) end end
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
