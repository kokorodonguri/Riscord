local S = settings({
  { key = "sentences", kind = "switch", default = true,
    label = { ja = "文ごとに大文字にする（オフなら先頭だけ）", en = "Capitalize every sentence (off: only the first letter)", zh = "每个句子都大写（关闭则只处理开头）" } },
}, {
  ja = "送信時に、文頭の英小文字を大文字にします。コマンド（/）、コードブロックを含む本文、URL の中は変えません。",
  en = "When sending, capitalizes lowercase letters at the start of sentences. Commands (/), messages with code blocks and URLs are left alone.",
  zh = "发送时将句首的小写英文字母改为大写。命令（/）、含代码块的消息以及 URL 内部不会更改。",
})
local function capitalize(text)
  if text:match("^%s*[/!$]") or text:find("```", 1, true) or text:find("`", 1, true) then return nil end
  local out = text:gsub("^(%s*)(%l)", function(sp, c) return sp .. c:upper() end)
  if S.state.sentences then
    out = out:gsub("([%.!?]%s+)(%l)", function(p, c) return p .. c:upper() end)
    out = out:gsub("(\n%s*)(%l)", function(p, c) return p .. c:upper() end)
  end
  return out
end
return plugin(S, { on_send = function(content) return capitalize(content) end })
