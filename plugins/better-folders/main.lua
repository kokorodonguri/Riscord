local S = settings({
  { key = "close_others", kind = "switch", default = true, label = { ja = "開いたらほかのフォルダを閉じる", en = "Close other folders on open", zh = "打开时关闭其他文件夹" } },
})
return plugin(S)
