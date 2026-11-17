local S = settings({
  { key = "mode", kind = "input", default = "random",
    label = { ja = "名前の付け方（random / timestamp / suffix）", en = "Naming (random / timestamp / suffix)", zh = "命名方式（random / timestamp / suffix）" } },
  { key = "length", kind = "number", default = 12,
    label = { ja = "random の文字数", en = "Length for random", zh = "random 的字符数" } },
}, {
  ja = "random はランダムな英数字、timestamp は Unix 時刻（ミリ秒）、suffix は元の名前の後ろにランダムな文字を付けます。拡張子は残します。",
  en = "random uses random letters and digits, timestamp uses the Unix time in milliseconds, and suffix appends random characters to the original name. Extensions are kept.",
  zh = "random 使用随机字母和数字，timestamp 使用 Unix 时间（毫秒），suffix 在原名称后追加随机字符。扩展名保持不变。",
})
return plugin(S)
