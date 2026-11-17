local S = settings({
  { key = "aliases", kind = "input", default = "brb=be right back; omw=on my way",
    label = { ja = "エイリアス（名前=置き換え; で区切る）", en = "Aliases (name=replacement, separated by ;)", zh = "别名（名称=替换内容，用 ; 分隔）" } },
  { key = "case", kind = "switch", default = false,
    label = { ja = "大文字と小文字を区別する", en = "Case sensitive", zh = "区分大小写" } },
}, {
  ja = "単語として一致したエイリアスだけを置き換えます。/alias 名前 置き換え でも追加できます。",
  en = "Only whole-word matches are replaced. You can also add one with /alias name replacement.",
  zh = "仅替换作为完整单词匹配的别名。也可以用 /alias 名称 替换内容 添加。",
})
local function escape(s) return (s:gsub("%p", "%%%0")) end
local function ci(s)
  return (s:gsub("%a", function(c) return "[" .. c:lower() .. c:upper() .. "]" end))
end
local function apply(content)
  local aliases = pairs_of(S.state.aliases)
  local changed = false
  for name, value in pairs(aliases) do
    if name ~= "" then
      local pat = escape(name)
      if not S.state.case then pat = ci(pat) end
      local n
      content, n = content:gsub("%f[%w_]" .. pat .. "%f[^%w_]", (value:gsub("%%", "%%%%")))
      if n > 0 then changed = true end
    end
  end
  if changed then return content end
end
return plugin(S, {
  on_send = function(content)
    if content:find("```", 1, true) then return nil end
    return apply(content)
  end,
  commands = {
    alias = function(args)
      local name, value = args:match("^(%S+)%s+(.+)$")
      if not name then return end
      local list = S.state.aliases or ""
      if list ~= "" then list = list .. "; " end
      S.state.aliases = list .. name .. "=" .. value
      riscord.save(S.state)
    end,
  },
})
