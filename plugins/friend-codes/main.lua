local S = settings({
  { kind = "button", action = "create", label = { ja = "フレンド招待を作ってコピー", en = "Create and copy a friend invite", zh = "创建并复制好友邀请" } },
}, { ja = "Discord のフレンド招待（discord.gg のリンク）を作ります。", en = "Creates a Discord friend invite (a discord.gg link).", zh = "创建 Discord 好友邀请（discord.gg 链接）。" })
return plugin(S)
