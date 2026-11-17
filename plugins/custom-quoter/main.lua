local S = settings({
  { key = "format", kind = "input", default = "{quote}\n{mention} ",
    label = { ja = "書式（{quote} {content} {author} {mention} {link} {date}。\\n で改行）", en = "Format ({quote} {content} {author} {mention} {link} {date}; \\n for a new line)", zh = "格式（{quote} {content} {author} {mention} {link} {date}；\\n 表示换行）" } },
}, {
  ja = "メッセージにマウスを重ねたときの「引用」で使います。{quote} は各行の先頭に > を付けた本文です。",
  en = "Used by \"Quote\" in the message hover toolbar. {quote} is the text with > at the start of each line.",
  zh = "用于消息悬停工具栏中的“引用”。{quote} 是每行开头加上 > 的正文。",
})
local function link(m)
  return "https://discord.com/channels/" .. (m.guild_id or "@me") .. "/" .. m.channel_id .. "/" .. m.id
end
return plugin(S, {
  message_actions = {
    { label = "CustomQuoter: " .. t({ ja = "引用", en = "Quote", zh = "引用" }), icon = "message-square", run = function(m)
      local quote = ("\n" .. m.content):gsub("\n", "\n> "):sub(2)
      local date = riscord.date("%Y-%m-%d %H:%M")
      local f = (S.state.format or "{quote}\n"):gsub("\\n", "\n")
      local vars = { quote = quote, content = m.content, author = m.author_name, mention = "<@" .. m.author_id .. ">", link = link(m), date = date }
      local out = f:gsub("{(%w+)}", function(k) return vars[k] end)
      riscord.insert(out)
    end },
  },
})
