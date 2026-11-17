local S = settings({
  { key = "always", kind = "input", default = "", label = { ja = "いつもすぐ読み込むチャンネル ID（, 区切り）", en = "Channel IDs to always load (comma separated)", zh = "始终立即加载的频道 ID（逗号分隔）" } },
})
return plugin(S)
