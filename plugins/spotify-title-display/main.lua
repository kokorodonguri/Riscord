local S = settings({
  { key = "both", kind = "switch", default = true, label = { ja = "アーティスト名も出す", en = "Also show the artist", zh = "也显示艺人名称" } },
})
return plugin(S)
