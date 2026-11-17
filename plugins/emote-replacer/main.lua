local S = settings({
  { key = "emotes", kind = "input", default = "", label = { ja = "エモート（名前=画像URL; で区切る）", en = "Emotes (name=image URL, separated by ;)", zh = "表情（名称=图片URL，用 ; 分隔）" } },
}, {
  ja = "本文の単語がエモート名と完全に一致したら、その画像の URL に置き換えます（Discord が画像として埋め込みます）。BetterTTV などの一覧は読み込みません。",
  en = "Words that exactly match an emote name are replaced with its image URL (Discord embeds it). Lists like BetterTTV aren't downloaded.",
  zh = "正文中与表情名称完全一致的词会被替换为图片 URL（Discord 会嵌入显示）。不会下载 BetterTTV 等列表。",
})
return plugin(S, {
  on_send = function(content)
    local emotes = pairs_of(S.state.emotes)
    if next(emotes) == nil or content:find("`", 1, true) then return nil end
    local out = content:gsub("%S+", function(w)
      local url = emotes[w]
      if url and url:match("^https?://") then return url end
    end)
    if out ~= content then return out end
  end,
})
