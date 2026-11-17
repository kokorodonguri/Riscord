local S = settings({
  { key = "link", kind = "switch", default = true, label = { ja = "元のメッセージへのリンクを付ける", en = "Add a link to the original message", zh = "附上原消息的链接" } },
})
return plugin(S, {
  message_actions = {
    { label = "Quoter: " .. t({ ja = "名言として引用", en = "Quote as a saying", zh = "作为名言引用" }), run = function(m)
      local when = m.timestamp:sub(1, 10)
      local out = "> “" .. m.content:gsub("\n", "\n> ") .. "”\n> — **" .. m.author_name .. "**, " .. when
      if S.state.link then
        out = out .. "\n> https://discord.com/channels/" .. (m.guild_id or "@me") .. "/" .. m.channel_id .. "/" .. m.id
      end
      riscord.insert(out .. "\n")
    end },
  },
})
