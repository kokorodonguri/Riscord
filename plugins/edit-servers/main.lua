local S = settings({
  { key = "names", kind = "input", default = "", label = { ja = "名前（サーバーID=名前; で区切る）", en = "Names (serverID=name, separated by ;)", zh = "名称（服务器ID=名称，用 ; 分隔）" } },
})
return plugin(S)
