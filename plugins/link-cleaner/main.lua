local S = settings({
  { key = "incoming", kind = "switch", default = false,
    label = { ja = "受信したメッセージの URL も表示時にきれいにする", en = "Also clean URLs in received messages when shown", zh = "显示收到的消息时也清理其中的 URL" } },
  { key = "extra", kind = "input", default = "",
    label = { ja = "追加で消すパラメーター名（, 区切り）", en = "Extra parameter names to remove (comma separated)", zh = "额外删除的参数名（用逗号分隔）" } },
})
local generic = { "^utm_", "^fbclid$", "^gclid$", "^dclid$", "^gbraid$", "^wbraid$", "^msclkid$", "^igshid$", "^igsh$",
  "^mc_cid$", "^mc_eid$", "^_hsenc$", "^_hsmi$", "^yclid$", "^ref_src$", "^ref_url$", "^oly_", "^vero_", "^twclid$",
  "^ttclid$", "^spm$", "^share_id$", "^mkt_tok$", "^trk$", "^__s$", "^ncid$", "^s_cid$", "^cmpid$", "^soc_src$", "^soc_trk$" }
local per_host = {
  ["youtube.com"] = { "^si$", "^feature$", "^pp$" },
  ["youtu.be"] = { "^si$", "^feature$" },
  ["spotify.com"] = { "^si$", "^context$", "^nd$" },
  ["twitter.com"] = { "^s$", "^t$" },
  ["x.com"] = { "^s$", "^t$" },
  ["amazon."] = { "^ref$", "^tag$", "^pd_rd_", "^pf_rd_", "^psc$", "^th$", "^linkCode$", "^content%-id$", "^qid$", "^sr$", "^keywords$", "^crid$", "^sprefix$" },
  ["reddit.com"] = { "^share_id$", "^rdt$" },
  ["tiktok.com"] = { "^_r$", "^_t$", "^is_from_webapp$", "^sender_device$", "^is_copy_url$" },
  ["instagram.com"] = { "^igsh$", "^igshid$", "^img_index$" },
  ["facebook.com"] = { "^mibextid$", "^rdid$", "^share_url$" },
}
local function clean(url)
  local base, query, frag = url:match("^([^?#]*)%??([^#]*)(#?.*)$")
  if not base or query == "" then return url end
  local host = (base:match("^https?://([^/]+)") or ""):lower()
  local rules = {}
  for _, r in ipairs(generic) do rules[#rules + 1] = r end
  for _, name in ipairs(split(S.state.extra)) do rules[#rules + 1] = "^" .. name:gsub("%p", "%%%0") .. "$" end
  for h, rs in pairs(per_host) do
    if host:find(h, 1, true) then
      for _, r in ipairs(rs) do rules[#rules + 1] = r end
    end
  end
  local kept = {}
  for pair in query:gmatch("[^&]+") do
    local key = pair:match("^([^=]*)")
    local drop = false
    for _, r in ipairs(rules) do
      if key:match(r) then drop = true break end
    end
    if not drop then kept[#kept + 1] = pair end
  end
  if #kept == 0 then return base .. frag end
  return base .. "?" .. table.concat(kept, "&") .. frag
end
local function clean_all(content)
  if not content:find("http", 1, true) then return nil end
  local out = content:gsub("https?://[^%s<>]+", clean)
  if out ~= content then return out end
end
return plugin(S, {
  on_send = function(content) return clean_all(content) end,
  on_display = function(content)
    if S.state.incoming then return clean_all(content) end
  end,
})
