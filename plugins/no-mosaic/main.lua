local S = settings({
  { key = "large", kind = "switch", default = false, label = { ja = "画像を大きめ（最大 640×480）に表示する", en = "Show images larger (up to 640×480)", zh = "以较大尺寸显示图片（最大 640×480）" } },
}, { ja = "Riscord はもともとタイル表示をしません。", en = "Riscord doesn't use the mosaic layout in the first place.", zh = "Riscord 本来就不使用拼贴布局。" })
return plugin(S)
