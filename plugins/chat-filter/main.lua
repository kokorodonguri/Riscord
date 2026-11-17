local S = settings({
  { key = "censor", kind = "input", default = "", label = { ja = "伏せ字にする言葉（, 区切り）", en = "Words to censor (comma separated)", zh = "要打码的词语（逗号分隔）" } },
  { key = "block", kind = "input", default = "", label = { ja = "メッセージごと隠す言葉（, 区切り）", en = "Words that hide the whole message (comma separated)", zh = "整条消息隐藏的词语（逗号分隔）" } },
  { key = "char", kind = "input", default = "*", label = { ja = "伏せ字の文字", en = "Censor character", zh = "打码字符" } },
  { key = "outgoing", kind = "switch", default = false, label = { ja = "送るメッセージも伏せ字にする", en = "Also censor messages you send", zh = "发送的消息也打码" } },
})
local function ci(s)
  s = s:gsub("%p", "%%%0")
  return (s:gsub("%a", function(c) return "[" .. c:lower() .. c:upper() .. "]" end))
end
local function censor(content)
  local changed = false
  local mark = S.state.char
  if mark == nil or mark == "" then mark = "*" end
  for _, word in ipairs(split(S.state.censor)) do
    local n
    content, n = content:gsub(ci(word), function(m) return mark:rep(utf8.len(m) or #m) end)
    if n > 0 then changed = true end
  end
  return content, changed
end
return plugin(S, {
  on_display = function(content)
    local lower = content:lower()
    for _, word in ipairs(split(S.state.block)) do
      if lower:find(word:lower(), 1, true) then
        return t({ ja = "（ChatFilter で隠したメッセージ）", en = "(Message hidden by ChatFilter)", zh = "（已被 ChatFilter 隐藏的消息）" })
      end
    end
    local out, changed = censor(content)
    if changed then return out end
  end,
  on_send = function(content)
    if not S.state.outgoing then return nil end
    local out, changed = censor(content)
    if changed then return out end
  end,
})
