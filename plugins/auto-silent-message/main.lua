local S = settings({}, {
  ja = "有効な間、送信するメッセージに @silent と同じフラグを付けます。プラグインがオフでも、本文の先頭に @silent と書けばその 1 通だけサイレントになります。",
  en = "While on, every message is sent with the same flag as @silent. Even when off, starting a message with @silent makes just that one silent.",
  zh = "启用期间，发送的每条消息都会带上与 @silent 相同的标记。即使关闭此插件，在正文开头写 @silent 也能让该条消息静默。",
})
return plugin(S)
