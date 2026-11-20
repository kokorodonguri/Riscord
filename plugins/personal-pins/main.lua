local S = settings({}, {
  ja = "メッセージのメニューの「自分用にピン留め」で保存します。下の一覧から開いたり外したりできます。",
  en = "Save with \"Pin for me\" in the message menu. Open or remove them from the list below.",
  zh = "通过消息菜单中的“为自己置顶”保存。可在下方列表中打开或移除。",
})
local state = S.state
state.pins = state.pins or {}
local page = 1
-- 1 件に部品を 3 つ使うので、部品の上限（100）に収まるようページに分ける
local PAGE_SIZE = 25
-- 保存できる大きさ（1 MiB）に余裕を持たせた、全体と 1 件の本文の上限（バイト）
local MAX_TOTAL = 700000
local MAX_BODY = 2000
-- UTF-8 の文字の途中で切らないよう、max バイト以内の文字の境目で切る
local function clip(s, max)
  if #s <= max then return s end
  local cut = utf8.offset(s, 0, max + 1) or (max + 1)
  return s:sub(1, cut - 1)
end
-- 保存したときのおおよその大きさ。エスケープされる文字は多めに数える
local function size(p)
  local escaped = select(2, p.content:gsub('[%c"\\]', ""))
  return #p.content + #(p.author or "") + escaped * 5 + 200
end
-- 古いものから捨てて、全体を保存できる大きさに収める
local function fit()
  local total = 0
  for _, p in ipairs(state.pins) do
    p.content = clip(p.content or "", MAX_BODY)
    p.size = p.size or size(p)
    total = total + p.size
  end
  while #state.pins > 500 or (total > MAX_TOTAL and #state.pins > 0) do
    total = total - table.remove(state.pins, 1).size
  end
end
fit()
S.extra_page = function()
  local items = {}
  local n = #state.pins
  if n == 0 then
    items[1] = { kind = "text", label = t({ ja = "まだありません。", en = "Nothing yet.", zh = "暂无。" }) }
    return items
  end
  local pages = math.max(1, math.ceil(n / PAGE_SIZE))
  page = math.max(1, math.min(page, pages))
  items[#items + 1] = { kind = "text", label = n .. t({ ja = " 件", en = " pins", zh = " 条" }) .. " (" .. page .. "/" .. pages .. ")" }
  if page > 1 then items[#items + 1] = { kind = "button", action = "prev_page", label = t({ ja = "前へ", en = "Previous", zh = "上一页" }) } end
  if page < pages then items[#items + 1] = { kind = "button", action = "next_page", label = t({ ja = "次へ", en = "Next", zh = "下一页" }) } end
  -- 新しいものから
  local first = n - (page - 1) * PAGE_SIZE
  for i = first, math.max(1, first - PAGE_SIZE + 1), -1 do
    local p = state.pins[i]
    local text = p.content
    if #text > 200 then text = clip(text, 200) .. "…" end
    items[#items + 1] = { kind = "text", label = p.author .. " · " .. (p.timestamp or ""):sub(1, 16):gsub("T", " ") .. "\n" .. text }
    items[#items + 1] = { kind = "button", action = "open:" .. i, label = t({ ja = "↑ を開く", en = "Open ↑", zh = "打开 ↑" }) }
    items[#items + 1] = { kind = "button", action = "del:" .. i, label = t({ ja = "↑ を外す", en = "Remove ↑", zh = "移除 ↑" }) }
  end
  return items
end
S.handle = function(action)
  if action == "prev_page" then page = math.max(1, page - 1) return end
  if action == "next_page" then page = page + 1 return end
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
      table.insert(state.pins, { id = m.id, channel_id = m.channel_id, guild_id = m.guild_id, author = m.author_name, content = clip(m.content, MAX_BODY), timestamp = m.timestamp })
      fit()
      riscord.save(state)
      riscord.notify("PersonalPins", t({ ja = "ピン留めしました", en = "Pinned", zh = "已置顶" }))
    end },
  },
})
