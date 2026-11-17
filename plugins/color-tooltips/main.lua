local function hsl(r, g, b)
  r, g, b = r / 255, g / 255, b / 255
  local max, min = math.max(r, g, b), math.min(r, g, b)
  local l = (max + min) / 2
  if max == min then return 0, 0, math.floor(l * 100 + 0.5) end
  local d = max - min
  local s = l > 0.5 and d / (2 - max - min) or d / (max + min)
  local h
  if max == r then h = (g - b) / d + (g < b and 6 or 0)
  elseif max == g then h = (b - r) / d + 2
  else h = (r - g) / d + 4 end
  return math.floor(h * 60 + 0.5), math.floor(s * 100 + 0.5), math.floor(l * 100 + 0.5)
end
local function convert(content)
  local lines = {}
  for hex in content:gmatch("#(%x%x%x%x%x%x)%f[^%w]") do
    local r, g, b = tonumber(hex:sub(1, 2), 16), tonumber(hex:sub(3, 4), 16), tonumber(hex:sub(5, 6), 16)
    local h, s, l = hsl(r, g, b)
    lines[#lines + 1] = string.format("#%s  rgb(%d, %d, %d)  hsl(%d, %d%%, %d%%)  %d", hex:lower(), r, g, b, h, s, l, tonumber(hex, 16))
  end
  for r, g, b in content:gmatch("rgb%(%s*(%d+)%s*,%s*(%d+)%s*,%s*(%d+)%s*%)") do
    r, g, b = tonumber(r), tonumber(g), tonumber(b)
    if r <= 255 and g <= 255 and b <= 255 then
      lines[#lines + 1] = string.format("rgb(%d, %d, %d)  #%02x%02x%02x", r, g, b, r, g, b)
    end
  end
  return lines
end
return {
  message_actions = {
    { label = "ColorTooltips: " .. t({ ja = "色を変換", en = "Convert colors", zh = "转换颜色" }), icon = "palette", run = function(m)
      local lines = convert(m.content)
      if #lines == 0 then
        riscord.notify("ColorTooltips", t({ ja = "色コードが見つかりません", en = "No color codes found", zh = "未找到颜色代码" }))
      else
        local text = table.concat(lines, "\n")
        riscord.copy(text)
        riscord.notify("ColorTooltips", text)
      end
    end },
  },
}
