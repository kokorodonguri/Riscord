local S = settings({}, { ja = "読み込んでいないメッセージは Discord から 1 件だけ読みます。押すとそのメッセージへ移ります。", en = "Messages not loaded yet are fetched one at a time. Click to jump there.", zh = "未加载的消息会单独获取一条。点击即可跳转。" })
return plugin(S)
