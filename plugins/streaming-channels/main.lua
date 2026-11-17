local S = settings({
  { key = "channels", kind = "input", default = "", label = { ja = "隠すチャンネル ID（, 区切り。空ならすべて）", en = "Channel IDs to hide (comma separated; empty: all)", zh = "要隐藏的频道 ID（逗号分隔；为空则全部）" } },
})
return plugin(S)
