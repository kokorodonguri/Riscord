local S = settings({
  { key = "react", kind = "input", default = "👍", label = { ja = "Ctrl+ダブルクリックで付けるリアクション", en = "Reaction for Ctrl+double-click", zh = "Ctrl+双击添加的反应" } },
}, {
  ja = "ダブルクリック: 自分のメッセージは編集、他人のメッセージは返信。Shift+ダブルクリック: 本文をコピー。Ctrl+ダブルクリック: リアクション。Alt+ダブルクリック: 入力欄に引用。",
  en = "Double-click: edit your messages, reply to others. Shift+double-click: copy text. Ctrl+double-click: react. Alt+double-click: quote into the input.",
  zh = "双击：编辑自己的消息、回复他人的消息。Shift+双击：复制正文。Ctrl+双击：添加反应。Alt+双击：引用到输入框。",
})
return plugin(S)
