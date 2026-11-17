local S = settings({}, {
  ja = "上限（無料 2000 文字・Nitro 4000 文字）を超えた本文は、改行や空白の位置で分けて順に送ります。コードブロックの途中で分けたときは閉じて開き直します。",
  en = "Messages over the limit (2000 characters, or 4000 with Nitro) are split at line breaks or spaces and sent in order. Code blocks split in the middle are closed and reopened.",
  zh = "超出上限（免费 2000 字、Nitro 4000 字）的正文会在换行或空格处拆分并依次发送。在代码块中间拆分时会先关闭再重新打开。",
})
return plugin(S)
