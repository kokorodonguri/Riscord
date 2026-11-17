local S = settings({}, {
  ja = "/remind 10m 洗濯物、/remind 18:30 会議、/remind 2026-12-24 09:00 プレゼント のように書きます。Riscord を閉じていた間に過ぎたものは、次に起動したときに知らせます。",
  en = "Write /remind 10m laundry, /remind 18:30 meeting or /remind 2026-12-24 09:00 presents. Reminders that passed while Riscord was closed fire the next time it starts.",
  zh = "可以写 /remind 10m 洗衣服、/remind 18:30 开会、/remind 2026-12-24 09:00 礼物。Riscord 关闭期间到期的提醒会在下次启动时通知。",
})
local state = S.state
state.items = state.items or {}
local function fire(item)
  for i, it in ipairs(state.items) do
    if it.id == item.id then table.remove(state.items, i) break end
  end
  riscord.save(state)
  riscord.notify(t({ ja = "リマインダー", en = "Reminder", zh = "提醒" }), item.text)
  riscord.play_sound("reminder")
end
local function schedule(item)
  local wait = item.at * 1000 - riscord.now()
  if wait < 100 then wait = 100 end
  if wait > 86400000 then wait = 86400000 end
  riscord.set_timeout(wait, function()
    if riscord.now() >= item.at * 1000 - 500 then fire(item) else schedule(item) end
  end)
end
S.extra_page = function()
  local items = {}
  if #state.items == 0 then
    items[1] = { kind = "text", label = t({ ja = "予定はありません。", en = "No reminders.", zh = "没有提醒。" }) }
  end
  for _, it in ipairs(state.items) do
    items[#items + 1] = { kind = "text", label = riscord.date("%Y-%m-%d %H:%M", it.at) .. "  " .. it.text }
    items[#items + 1] = { kind = "button", action = "del:" .. it.id, label = t({ ja = "↑ を削除", en = "Delete ↑", zh = "删除 ↑" }) }
  end
  items[#items + 1] = { kind = "input", action = "new", label = t({ ja = "新しいリマインダー（例: 30m 休憩）", en = "New reminder (e.g. 30m break)", zh = "新提醒（例：30m 休息）" }) }
  items[#items + 1] = { kind = "button", action = "add", label = t({ ja = "追加", en = "Add", zh = "添加" }) }
  return items
end
local function add(text)
  local at, rest = parse_when(text)
  if not at then return false end
  rest = trim(rest or "")
  if rest == "" then rest = t({ ja = "時間です", en = "Time's up", zh = "时间到了" }) end
  local item = { id = tostring(riscord.now()), at = at, text = rest }
  table.insert(state.items, item)
  riscord.save(state)
  schedule(item)
  return true
end
S.handle = function(action, values)
  local id = action:match("^del:(.+)$")
  if id then
    for i, it in ipairs(state.items) do
      if it.id == id then table.remove(state.items, i) break end
    end
    riscord.save(state)
  elseif action == "add" then
    add(values.new or "")
  end
end
return plugin(S, {
  start = function()
    for _, it in ipairs(state.items) do schedule(it) end
  end,
  commands = {
    remind = function(args)
      if not add(args) then
        riscord.notify("Reminder", t({ ja = "時刻が読めませんでした: " .. args, en = "Couldn't read the time: " .. args, zh = "无法识别时间：" .. args }))
      end
      return true
    end,
  },
})
