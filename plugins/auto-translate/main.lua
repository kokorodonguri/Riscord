local S = settings({
  { key = "only_channels", kind = "input", default = "", label = { ja = "対象のチャンネル ID（, 区切り。空なら開いているチャンネル）", en = "Channel IDs (comma separated; empty: the open channel)", zh = "目标频道 ID（逗号分隔；为空则为当前打开的频道）" } },
  { key = "skip_users", kind = "input", default = "", label = { ja = "翻訳しないユーザー ID（, 区切り）", en = "User IDs not to translate (comma separated)", zh = "不翻译的用户 ID（逗号分隔）" } },
}, {
  ja = "翻訳先は Translator プラグインのページで決めた言語です。本文は Google 翻訳へ送信されます。Translator も有効にしてください。",
  en = "The target language is the one set on the Translator plugin's page. Text is sent to Google Translate. Turn on Translator as well.",
  zh = "目标语言为 Translator 插件页面中设置的语言。正文会发送到 Google 翻译。请同时启用 Translator。",
})
local function listed(list, value)
  for _, v in ipairs(split(list)) do if v == value then return true end end
  return false
end
return plugin(S, {
  should_translate = function(channel_id, author_id)
    if listed(S.state.skip_users, author_id) then return false end
    if S.state.only_channels == "" then return channel_id == riscord.me().channel_id end
    return listed(S.state.only_channels, channel_id)
  end,
})
