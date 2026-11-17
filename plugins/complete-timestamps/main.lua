local S = settings({
  { key = "format", kind = "input", default = "%Y-%m-%d %H:%M:%S", label = { ja = "通常表示の書式（strftime）", en = "Format (strftime)", zh = "常规显示格式（strftime）" } },
  { key = "compact", kind = "input", default = "%H:%M:%S", label = { ja = "コンパクト表示・連投の書式", en = "Format for compact mode and grouped messages", zh = "紧凑模式与连续消息的格式" } },
  { key = "always", kind = "switch", default = false, label = { ja = "連投したメッセージにも常に時刻を出す", en = "Always show the time on grouped messages", zh = "连续消息也始终显示时间" } },
}, { ja = "例: %Y年%m月%d日 %H:%M:%S、%a %H:%M", en = "Examples: %Y-%m-%d %H:%M:%S, %a %I:%M %p", zh = "示例：%Y年%m月%d日 %H:%M:%S、%a %H:%M" })
return plugin(S)
