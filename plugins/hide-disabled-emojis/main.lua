local S = settings({}, { ja = "Riscord はもともと、Nitro がないと使えない絵文字やサーバーで無効になった絵文字をピッカーに出しません（FreeEmojis が有効なときを除く）。", en = "Riscord already leaves out emojis you can't use without Nitro and emojis disabled by the server (unless FreeEmojis is on).", zh = "Riscord 本来就不会在选择器中显示没有 Nitro 无法使用的表情以及被服务器禁用的表情（启用 FreeEmojis 时除外）。" })
return plugin(S)
