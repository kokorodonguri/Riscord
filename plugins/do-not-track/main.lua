local S = settings({}, {
  ja = "Riscord は Discord の解析（science）や Sentry への送信を実装していないため、このプラグインの有無に関係なく送りません。公式アプリと揃えるための項目です。",
  en = "Riscord doesn't implement Discord analytics (science) or Sentry reporting, so nothing is sent with or without this plugin. It exists to match the official app's plugin.",
  zh = "Riscord 未实现 Discord 分析（science）或 Sentry 上报，因此无论是否启用此插件都不会发送。此项仅为与原插件对应。",
})
return plugin(S)
