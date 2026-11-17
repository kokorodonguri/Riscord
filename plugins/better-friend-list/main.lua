local S = settings({
  { key = "by_status", kind = "switch", default = false, label = { ja = "オンライン状態の順に並べる", en = "Sort by status", zh = "按在线状态排序" } },
}, { ja = "左上の DM 検索欄に打った文字で絞り込みます。", en = "Filters with the text in the DM search box at the top left.", zh = "使用左上角私信搜索框中的文字进行筛选。" })
return plugin(S)
