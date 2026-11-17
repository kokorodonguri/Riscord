local S = settings({
  { key = "no_users", kind = "switch", default = false, label = { ja = "ユーザーへのメンションで通知しない", en = "Don't notify mentioned users", zh = "提及用户时不通知" } },
  { key = "no_roles", kind = "switch", default = false, label = { ja = "ロールへのメンションで通知しない", en = "Don't notify mentioned roles", zh = "提及身份组时不通知" } },
  { key = "no_everyone", kind = "switch", default = true, label = { ja = "@everyone・@here で通知しない", en = "Don't notify @everyone / @here", zh = "@everyone / @here 不通知" } },
  { key = "no_reply", kind = "switch", default = false, label = { ja = "返信先へ通知しない", en = "Don't notify the replied user", zh = "不通知被回复的人" } },
}, {
  ja = "メンションは本文に残り、通知だけを止めます。",
  en = "Mentions stay in the text; only the notifications are suppressed.",
  zh = "提及仍保留在正文中，只是不会发送通知。",
})
return plugin(S)
