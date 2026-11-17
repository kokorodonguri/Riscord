local S = settings({
  { key = "nsfw_channels", kind = "switch", default = true, label = { ja = "NSFW のチャンネルで隠す", en = "Hide in NSFW channels", zh = "在 NSFW 频道中隐藏" } },
  { key = "channels", kind = "input", default = "", label = { ja = "ほかに隠すチャンネル ID（, 区切り）", en = "Other channel IDs (comma separated)", zh = "其他要隐藏的频道 ID（逗号分隔）" } },
})
return plugin(S)
