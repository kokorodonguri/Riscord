local S = settings({
  { key = "size", kind = "number", default = 48, label = { ja = "画像の大きさ（16〜128）", en = "Image size (16–128)", zh = "图片大小（16–128）" } },
}, {
  ja = "絵文字ピッカーと :名前: の補完に全サーバーの絵文字が出るようになります。使えない絵文字は、送るときに画像の URL に置き換えます。",
  en = "The emoji picker and :name: completion show emojis from every server. Emojis you can't use are replaced with image URLs when sending.",
  zh = "表情选择器和 :名称: 补全会显示所有服务器的表情。无法使用的表情会在发送时替换为图片 URL。",
})
return plugin(S)
