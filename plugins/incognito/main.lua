local S = settings({
  { key = "hide_typing", kind = "switch", default = true, label = { ja = "「入力中…」を送らない", en = "Don't send typing", zh = "不发送“正在输入…”" } },
  { key = "no_read_ack", kind = "switch", default = false, label = { ja = "既読を送らない（未読表示は手元だけ消す）", en = "Don't send read receipts (unread marks clear locally only)", zh = "不发送已读（未读标记只在本地清除）" } },
  { key = "hide_activity", kind = "switch", default = false, label = { ja = "プレイ中などのアクティビティを出さない", en = "Don't show activities like \"Playing\"", zh = "不显示“正在玩”等活动" } },
  { key = "generic_client", kind = "switch", default = false, label = { ja = "接続時の端末情報を一般的なブラウザーにする（次の接続から）", en = "Report a generic web browser when connecting (from the next connection)", zh = "连接时报告为通用浏览器（下次连接起生效）" } },
}, {
  ja = "Riscord はもともと Sentry や解析用の送信をしません。",
  en = "Riscord never sends Sentry or analytics data in the first place.",
  zh = "Riscord 本来就不会发送 Sentry 或分析数据。",
})
return plugin(S)
