local function link(m)
  return "https://discord.com/channels/" .. (m.guild_id or "@me") .. "/" .. m.channel_id .. "/" .. m.id
end
local L = function(ja, en, zh) return "Copier: " .. t({ ja = ja, en = en, zh = zh }) end
return {
  message_actions = {
    { label = L("リンクをコピー", "Copy link", "复制链接"), run = function(m) riscord.copy(link(m)) end },
    { label = L("投稿者 ID をコピー", "Copy author ID", "复制作者 ID"), run = function(m) riscord.copy(m.author_id) end },
    { label = L("投稿者のメンションをコピー", "Copy author mention", "复制作者提及"), run = function(m) riscord.copy("<@" .. m.author_id .. ">") end },
    { label = L("引用の形でコピー", "Copy as quote", "以引用格式复制"), run = function(m)
      riscord.copy("> " .. m.content:gsub("\n", "\n> ") .. "\n— " .. m.author_name)
    end },
    { label = L("添付ファイルの URL をコピー", "Copy attachment URLs", "复制附件 URL"), run = function(m)
      riscord.copy(table.concat(m.attachments, "\n"))
    end },
    { label = L("本文をコードブロックでコピー", "Copy in a code block", "以代码块复制"), run = function(m)
      riscord.copy("```\n" .. m.content .. "\n```")
    end },
  },
}
