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
  local function transform(source, pattern)
    return (source:gsub(pattern, function(prefix, pos, c)
      local word = source:sub(pos):match("^%S+")
      -- URL やドメイン（www.example.com、example.com/path）は大文字小文字を区別することがあるので変えない
      if word:find("://", 1, true) or word:match("^[%w%-]+%.[%w%-]*%a[%w%-]*") then return prefix .. c end
      return prefix .. c:upper()
    end))
  end
  local out = transform(text, "^(%s*)()(%l)")
  if S.state.sentences then
    out = transform(out, "([%.!?]%s+)()(%l)")
    out = transform(out, "(\n%s*)()(%l)")
  end
  return out
end
return plugin(S, { on_send = function(content) return capitalize(content) end })
