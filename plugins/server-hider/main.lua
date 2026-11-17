local S = settings({
  { key = "hidden", kind = "input", default = "", label = { ja = "隠すサーバー ID（, 区切り。右クリックの「サーバーを隠す」でも追加）", en = "Server IDs to hide (comma separated; also via Hide Server in the menu)", zh = "要隐藏的服务器 ID（逗号分隔；也可通过右键菜单“隐藏服务器”添加）" } },
})
S.handle = function(action)
  local id = action:match("^toggle:(%d+)$")
  if not id then return end
  local items, found = {}, false
  for _, v in ipairs(split(S.state["hidden"])) do
    if v == id then found = true else items[#items + 1] = v end
  end
  if not found then items[#items + 1] = id end
  S.state["hidden"] = table.concat(items, ",")
  riscord.save(S.state)
end
return plugin(S)
