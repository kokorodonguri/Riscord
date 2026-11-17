local S = settings({
  { key = "pinned", kind = "input", default = "", label = { ja = "固定する DM のチャンネル ID（, 区切り）", en = "DM channel IDs to pin (comma separated)", zh = "要固定的私信频道 ID（逗号分隔）" } },
})
S.handle = function(action)
  local id = action:match("^toggle:(%d+)$")
  if not id then return end
  local items, found = {}, false
  for _, v in ipairs(split(S.state["pinned"])) do
    if v == id then found = true else items[#items + 1] = v end
  end
  if not found then items[#items + 1] = id end
  S.state["pinned"] = table.concat(items, ",")
  riscord.save(S.state)
end
return plugin(S)
