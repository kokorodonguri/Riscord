local S = settings({}, {
  ja = "返信の「メンション」をオフにして送ります。本文に @ を書いた相手には通知されます。",
  en = "Replies are sent with the reply mention turned off. People you @ in the text are still notified.",
  zh = "以关闭“提及”的方式发送回复。正文中 @ 的对象仍会收到通知。",
})
return plugin(S)
