local S = settings({}, {
  ja = "メッセージのメニューの「自分用にピン留め」で保存します。下の一覧から開いたり外したりできます。",
  en = "Save with \"Pin for me\" in the message menu. Open or remove them from the list below.",
  zh = "通过消息菜单中的“为自己置顶”保存。可在下方列表中打开或移除。",
})
local state = S.state
state.pins = state.pins or {}
S.extra_page = function()
  local items = {}
  if #state.pins == 0 then
    items[1] = { kind = "text", label = t({ ja = "まだありません。", en = "Nothing yet.", zh = "暂无。" }) }
  end
  for i = #state.pins, 1, -1 do
    local p = state.pins[i]
    local text = p.content
    if #text > 200 then text = text:sub(1, 200) .. "…" end
    items[#items + 1] = { kind = "text", label = p.author .. " · " .. (p.timestamp or ""):sub(1, 16):gsub("T", " ") .. "\n" .. text }
    items[#items + 1] = { kind = "button", action = "open:" .. i, label = t({ ja = "↑ を開く", en = "Open ↑", zh = "打开 ↑" }) }
    items[#items + 1] = { kind = "button", action = "del:" .. i, label = t({ ja = "↑ を外す", en = "Remove ↑", zh = "移除 ↑" }) }
  end
  return items
end
S.handle = function(action)
  local kind, i = action:match("^(%a+):(%d+)$")
  i = tonumber(i)
  local p = i and state.pins[i]
  if not p then return end
  if kind == "open" then
    riscord.open_channel(p.channel_id)
  elseif kind == "del" then
    table.remove(state.pins, i)
    riscord.save(state)
  end
end
return plugin(S, {
  message_actions = {
    { label = "PersonalPins: " .. t({ ja = "自分用にピン留め", en = "Pin for me", zh = "为自己置顶" }), icon = "pin", run = function(m)
      for _, p in ipairs(state.pins) do
        if p.id == m.id then return end
      end
      table.insert(state.pins, { id = m.id, channel_id = m.channel_id, guild_id = m.guild_id, author = m.author_name, content = m.content, timestamp = m.timestamp })
      while #state.pins > 500 do table.remove(state.pins, 1) end
      riscord.save(state)
      riscord.notify("PersonalPins", t({ ja = "ピン留めしました", en = "Pinned", zh = "已置顶" }))
    end },
  },
})
