local S = settings({
  { key = "hide_online", kind = "switch", default = false, label = { ja = "オンライン数は出さない", en = "Don't show the online count", zh = "不显示在线人数" } },
})
return plugin(S)
