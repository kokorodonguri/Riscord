local S = settings({}, {
  ja = "コマンド: /fw 全角、/sc 小型大文字、/up 上下反転、/leet、/mock 交互、/sup 上付き、/bubble 丸囲み。本文の中では [fw]…[/fw] のように囲むとその部分だけ変えます（タグ名はコマンドと同じ）。",
  en = "Commands: /fw fullwidth, /sc small caps, /up upside down, /leet, /mock alternating, /sup superscript, /bubble circled. Inside a message, wrap text like [fw]…[/fw] to change just that part (tags match the commands).",
  zh = "命令：/fw 全角、/sc 小型大写、/up 上下颠倒、/leet、/mock 大小写交替、/sup 上标、/bubble 圆圈字母。在正文中用 [fw]…[/fw] 包裹即可只转换该部分（标签名与命令相同）。",
})
local function map_chars(text, f)
  local out = {}
  for _, c in utf8.codes(text) do out[#out + 1] = f(c) end
  return table.concat(out)
end
local small = "ᴀʙᴄᴅᴇꜰɢʜɪᴊᴋʟᴍɴᴏᴘǫʀsᴛᴜᴠᴡxʏᴢ"
local small_t = {}
for _, c in utf8.codes(small) do small_t[#small_t + 1] = utf8.char(c) end
local up_map = { a="ɐ", b="q", c="ɔ", d="p", e="ǝ", f="ɟ", g="ƃ", h="ɥ", i="ᴉ", j="ɾ", k="ʞ", l="l", m="ɯ", n="u", o="o", p="d", q="b", r="ɹ", s="s", t="ʇ", u="n", v="ʌ", w="ʍ", x="x", y="ʎ", z="z",
  A="∀", B="ꓭ", C="Ɔ", D="ꓷ", E="Ǝ", F="Ⅎ", G="⅁", H="H", I="I", J="ſ", K="ꓘ", L="˥", M="W", N="N", O="O", P="Ԁ", Q="Ό", R="ꓤ", S="S", T="⊥", U="∩", V="Λ", W="M", X="X", Y="⅄", Z="Z",
  ["1"]="Ɩ", ["2"]="ᄅ", ["3"]="Ɛ", ["4"]="ㄣ", ["5"]="ϛ", ["6"]="9", ["7"]="ㄥ", ["8"]="8", ["9"]="6", ["0"]="0",
  ["."]="˙", [","]="'", ["'"]=",", ["!"]="¡", ["?"]="¿", ["("]=")", [")"]="(", ["["]="]", ["]"]="[", ["{"]="}", ["}"]="{", ["<"]=">", [">"]="<", ["_"]="‾", ["&"]="⅋" }
local sup_map = { a="ᵃ", b="ᵇ", c="ᶜ", d="ᵈ", e="ᵉ", f="ᶠ", g="ᵍ", h="ʰ", i="ⁱ", j="ʲ", k="ᵏ", l="ˡ", m="ᵐ", n="ⁿ", o="ᵒ", p="ᵖ", r="ʳ", s="ˢ", t="ᵗ", u="ᵘ", v="ᵛ", w="ʷ", x="ˣ", y="ʸ", z="ᶻ",
  ["0"]="⁰", ["1"]="¹", ["2"]="²", ["3"]="³", ["4"]="⁴", ["5"]="⁵", ["6"]="⁶", ["7"]="⁷", ["8"]="⁸", ["9"]="⁹", ["+"]="⁺", ["-"]="⁻", ["="]="⁼", ["("]="⁽", [")"]="⁾" }
local leet_map = { a="4", e="3", i="1", o="0", s="5", t="7", g="9", b="8", A="4", E="3", I="1", O="0", S="5", T="7", G="9", B="8" }
local styles = {
  fw = function(s) return map_chars(s, function(c)
    if c == 32 then return utf8.char(0x3000) end
    if c >= 0x21 and c <= 0x7E then return utf8.char(c + 0xFEE0) end
    return utf8.char(c) end) end,
  sc = function(s) return map_chars(s, function(c)
    if c >= 97 and c <= 122 then return small_t[c - 96] end
    return utf8.char(c) end) end,
  up = function(s)
    local chars = {}
    for _, c in utf8.codes(s) do
      local ch = utf8.char(c)
      table.insert(chars, 1, up_map[ch] or ch)
    end
    return table.concat(chars)
  end,
  leet = function(s) return (s:gsub("%a", function(c) return leet_map[c] end)) end,
  mock = function(s)
    local upper = false
    return (s:gsub("%a", function(c) upper = not upper; return upper and c:upper() or c:lower() end))
  end,
  sup = function(s) return map_chars(s, function(c) local ch = utf8.char(c); return sup_map[ch:lower()] or ch end) end,
  bubble = function(s) return map_chars(s, function(c)
    if c >= 97 and c <= 122 then return utf8.char(0x24D0 + c - 97) end
    if c >= 65 and c <= 90 then return utf8.char(0x24B6 + c - 65) end
    if c >= 49 and c <= 57 then return utf8.char(0x2460 + c - 49) end
    if c == 48 then return utf8.char(0x24EA) end
    return utf8.char(c) end) end,
}
local commands = {}
for name, f in pairs(styles) do
  commands[name] = function(args, channel)
    if trim(args) ~= "" then riscord.send_message(channel, f(args)) end
    return true
  end
end
return plugin(S, {
  commands = commands,
  on_send = function(content)
    if not content:find("[/", 1, true) then return nil end
    local out = content:gsub("%[(%a+)%](.-)%[/%1%]", function(tag, body)
      local f = styles[tag]
      if f then return f(body) end
    end)
    if out ~= content then return out end
  end,
})
