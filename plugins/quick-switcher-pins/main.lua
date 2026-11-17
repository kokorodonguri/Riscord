local S = settings({
  { key = "pins", kind = "input", default = "", label = { ja = "固定するチャンネル・サーバー ID（, 区切り）", en = "Channel or server IDs to pin (comma separated)", zh = "要固定的频道或服务器 ID（逗号分隔）" } },
})
return plugin(S)
