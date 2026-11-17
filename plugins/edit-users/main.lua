local S = settings({
  { key = "names", kind = "input", default = "", label = { ja = "名前（ユーザーID=名前; で区切る）", en = "Names (userID=name, separated by ;)", zh = "名称（用户ID=名称，用 ; 分隔）" } },
  { key = "colors", kind = "input", default = "", label = { ja = "色（ユーザーID=#rrggbb; で区切る）", en = "Colors (userID=#rrggbb, separated by ;)", zh = "颜色（用户ID=#rrggbb，用 ; 分隔）" } },
}, { ja = "変えた名前と色はこのパソコンの Riscord でだけ見えます。", en = "Changes are only visible in Riscord on this computer.", zh = "更改只在这台电脑的 Riscord 中可见。" })
return plugin(S)
