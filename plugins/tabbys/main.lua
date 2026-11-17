local S = settings({
  { key = "bookmarks", kind = "input", default = "", label = { ja = "ブックマークするチャンネル ID（, 区切り）", en = "Channel IDs to bookmark (comma separated)", zh = "要加书签的频道 ID（逗号分隔）" } },
})
return plugin(S)
