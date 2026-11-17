local S = settings({
  { key = "page", kind = "input", default = "friends", label = { ja = "開く場所（friends かチャンネル ID）", en = "Page to open (friends or a channel ID)", zh = "要打开的页面（friends 或频道 ID）" } },
})
return plugin(S)
