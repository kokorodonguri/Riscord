local S = settings({
  { key = "zones", kind = "input", default = "", label = { ja = "時差（ユーザーID=+9 や -5:30; で区切る）", en = "Offsets (userID=+9 or -5:30, separated by ;)", zh = "时差（用户ID=+9 或 -5:30，用 ; 分隔）" } },
})
return plugin(S, {
  message_actions = {
    { label = "Timezones: " .. t({ ja = "この人の ID を入力欄へ", en = "Put this user's ID in the input", zh = "将此人的 ID 放入输入框" }), icon = "clock", run = function(m) riscord.insert(m.author_id .. "=") end },
  },
})
