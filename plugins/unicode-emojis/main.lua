local S = settings({
  { key = "emoticons", kind = "switch", default = true,
    label = { ja = "顔文字（:) :D <3 など）も絵文字にする", en = "Also convert emoticons (:) :D <3 etc.)", zh = "也转换颜文字（:) :D <3 等）" } },
}, {
  ja = "Riscord は :名前: を送信時に Unicode の絵文字へ置き換えます（Discord の絵文字コードにしません）。このプラグインは顔文字の変換を足します。",
  en = "Riscord already sends :names: as Unicode emoji rather than Discord emoji codes. This plugin also converts emoticons.",
  zh = "Riscord 发送时已会把 :名称: 替换为 Unicode 表情（而不是 Discord 表情代码）。此插件额外转换颜文字。",
})
local map = {
  [":)"] = "🙂", [":-)"] = "🙂", [":D"] = "😄", [":-D"] = "😄", [";)"] = "😉", [";-)"] = "😉",
  [":("] = "🙁", [":-("] = "🙁", [":P"] = "😛", [":p"] = "😛", ["<3"] = "❤️", ["</3"] = "💔",
  [":O"] = "😮", [":o"] = "😮", ["xD"] = "😆", ["XD"] = "😆", [":'("] = "😢", [":|"] = "😐",
  ["B)"] = "😎", [":/"] = "😕", [">:("] = "😠", [":*"] = "😘", ["o/"] = "👋", ["^^"] = "😊",
}
return plugin(S, {
  on_send = function(content)
    if not S.state.emoticons or content:find("`", 1, true) then return nil end
    local out = content:gsub("%S+", function(w) return map[w] end)
    if out ~= content then return out end
  end,
})
