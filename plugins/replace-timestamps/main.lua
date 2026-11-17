local S = settings({
  { key = "style_time", kind = "input", default = "t", label = { ja = "時刻だけのときの表示（t T R など）", en = "Style for times only (t, T, R…)", zh = "仅时间时的样式（t、T、R 等）" } },
  { key = "style_full", kind = "input", default = "f", label = { ja = "日付と時刻のときの表示（f F R など）", en = "Style for date and time (f, F, R…)", zh = "日期和时间时的样式（f、F、R 等）" } },
}, {
  ja = "時刻はこのパソコンの地域の時刻として読みます。コードブロック・URL・すでにある <t:…> は変えません。",
  en = "Times are read in this computer's time zone. Code blocks, URLs and existing <t:…> are left alone.",
  zh = "时间按这台电脑的时区解读。代码块、URL 以及已有的 <t:…> 不会更改。",
})
local function style(key, fallback)
  local s = S.state[key]
  if s and s:match("^[tTdDfFR]$") then return s end
  return fallback
end
return plugin(S, {
  on_send = function(content)
    if content:find("`", 1, true) or content:find("<t:", 1, true) then return nil end
    local out = content:gsub("(%f[%d]%d%d%d%d[-/]%d%d?[-/]%d%d?%s+%d%d?:%d%d)%f[%D]", function(s)
      local at = parse_when(s)
      if at then return "<t:" .. at .. ":" .. style("style_full", "f") .. ">" end
    end)
    out = out:gsub("([%s%p]?)(%f[%d]%d%d?:%d%d%s*[aApP][mM])%f[%W]", function(pre, s)
      if pre == "/" or pre == ":" then return nil end
      local at = parse_when(s)
      if at then return pre .. "<t:" .. at .. ":" .. style("style_time", "t") .. ">" end
    end)
    out = out:gsub("(%S*)(%f[%d]%d%d?:%d%d)%f[^%d:]", function(pre, s)
      if pre:find("http", 1, true) or pre:find("<t", 1, true) or pre:match("[%d:/]$") then return nil end
      local h, m = s:match("(%d+):(%d+)")
      if tonumber(h) > 23 or tonumber(m) > 59 then return nil end
      local at = parse_when(s)
      if at then return pre .. "<t:" .. at .. ":" .. style("style_time", "t") .. ">" end
    end)
    if out ~= content then return out end
  end,
})
