local S = settings({
  { key = "max", kind = "number", default = 10, label = { ja = "タブの最大数", en = "Maximum tabs", zh = "最大标签数" } },
})
return plugin(S)
