local S = settings({
  { key = "ellipsis", kind = "switch", default = true,
    label = { ja = "… を ........ にする", en = "Turn … into ........", zh = "把 … 变成 ........" } },
}, {
  ja = "送信時に great→gr8、wait→w8、b→8 のような 8 の言い換えをします。",
  en = "When sending, swaps sounds for 8 like great→gr8, wait→w8, b→8.",
  zh = "发送时进行 great→gr8、wait→w8、b→8 之类的 8 替换。",
})
local sounds = { { "eight", "8" }, { "ate", "8" }, { "eat", "8" }, { "ait", "8" }, { "ight", "8" }, { "Eight", "8" }, { "Ate", "8" } }
local function quirk_word(w)
  if w:match("^https?://") or w:match("^<") or w:match("^:%w+:$") then return nil end
  for _, s in ipairs(sounds) do w = w:gsub(s[1], s[2]) end
  w = w:gsub("[Bb]", "8")
  return w
end
return plugin(S, {
  on_send = function(content)
    if content:find("```", 1, true) or content:match("^/") then return nil end
    local out = content:gsub("%S+", quirk_word)
    if S.state.ellipsis then
      out = out:gsub("…", "........"):gsub("%.%.%.+", "........")
    end
    out = out:gsub(":%)", "::::)")
    return out
  end,
})
