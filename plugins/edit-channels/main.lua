local S = settings({
  { key = "names", kind = "input", default = "", label = { ja = "名前（チャンネルID=名前; で区切る）", en = "Names (channelID=name, separated by ;)", zh = "名称（频道ID=名称，用 ; 分隔）" } },
})
return plugin(S)
