local S = settings({
  { key = "always", kind = "switch", default = false, label = { ja = "表示名と同じでも出す", en = "Show even when it matches the display name", zh = "与显示名称相同时也显示" } },
})
return plugin(S)
