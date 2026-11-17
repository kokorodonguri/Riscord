local S = settings({
  { key = "emojis", kind = "input", default = "👍,❤️,😂,😮,😢,🎉", label = { ja = "絵文字（, 区切り。最大 10 個）", en = "Emojis (comma separated, up to 10)", zh = "表情（逗号分隔，最多 10 个）" } },
})
return plugin(S)
