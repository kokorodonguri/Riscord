local S = settings({
  { key = "keywords", kind = "input", default = "", label = { ja = "言葉（, 区切り。大文字小文字は区別しない）", en = "Keywords (comma separated, case-insensitive)", zh = "关键词（逗号分隔，不区分大小写）" } },
  { key = "ignore_users", kind = "input", default = "", label = { ja = "無視するユーザー ID（, 区切り）", en = "User IDs to ignore (comma separated)", zh = "忽略的用户 ID（逗号分隔）" } },
  { key = "only_guilds", kind = "input", default = "", label = { ja = "対象のサーバー ID（空なら全部）", en = "Server IDs to watch (empty: all)", zh = "要关注的服务器 ID（为空则全部）" } },
  { key = "sound", kind = "switch", default = true, label = { ja = "音を鳴らす", en = "Play a sound", zh = "播放声音" } },
  { key = "bots", kind = "switch", default = false, label = { ja = "Bot のメッセージも対象にする", en = "Include bot messages", zh = "也包括机器人的消息" } },
})
local function listed(list, value)
  for _, v in ipairs(split(list)) do if v == value then return true end end
  return false
end
return plugin(S, {
  on_message = function(m)
    if m.author_id == riscord.me().id or (m.bot and not S.state.bots) then return end
    if listed(S.state.ignore_users, m.author_id) then return end
    if S.state.only_guilds ~= "" and not listed(S.state.only_guilds, m.guild_id or "") then return end
    local lower = m.content:lower()
    for _, word in ipairs(split(S.state.keywords)) do
      if lower:find(word:lower(), 1, true) then
        riscord.notify(m.author_name .. " (" .. word .. ")", m.content)
        if S.state.sound then riscord.play_sound("mention") end
        return
      end
    end
  end,
})
