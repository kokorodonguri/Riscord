local S = settings({
  { key = "collapsed", kind = "switch", default = false, label = { ja = "最初から畳んでおく", en = "Collapsed by default", zh = "默认折叠" } },
})
return plugin(S)
