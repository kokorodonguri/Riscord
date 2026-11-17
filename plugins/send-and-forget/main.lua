local S = settings({}, {
  ja = "Riscord にはまだメッセージの転送がないため、転送先へ移ることはありません。転送に対応したときもこの設定に従います。",
  en = "Riscord doesn't support forwarding messages yet, so it never follows them. This setting will apply once forwarding is supported.",
  zh = "Riscord 尚不支持转发消息，因此不会跳转到目标频道。支持转发后也会遵循此设置。",
})
return plugin(S)
