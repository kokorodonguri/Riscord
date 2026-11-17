local S = settings({
  { key = "mute", kind = "switch", default = false, label = { ja = "ミュートする", en = "Mute the server", zh = "静音服务器" } },
  { key = "hide_muted", kind = "switch", default = false, label = { ja = "ミュートしたチャンネルを隠す", en = "Hide muted channels", zh = "隐藏已静音频道" } },
  { key = "notify", kind = "input", default = "", label = { ja = "通知（all / mentions / none。空なら変えない）", en = "Notifications (all / mentions / none; empty: unchanged)", zh = "通知（all / mentions / none；为空则不更改）" } },
})
return plugin(S)
