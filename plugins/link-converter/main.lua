local rules = {
  { key = "twitter", hosts = { "twitter.com", "x.com" }, default = "fxtwitter.com", label = "Twitter / X" },
  { key = "instagram", hosts = { "instagram.com" }, default = "ddinstagram.com", label = "Instagram" },
  { key = "tiktok", hosts = { "tiktok.com" }, default = "vxtiktok.com", label = "TikTok" },
  { key = "reddit", hosts = { "reddit.com" }, default = "rxddit.com", label = "Reddit" },
  { key = "bluesky", hosts = { "bsky.app" }, default = "bskx.app", label = "Bluesky" },
  { key = "pixiv", hosts = { "pixiv.net" }, default = "phixiv.net", label = "pixiv" },
  { key = "threads", hosts = { "threads.net" }, default = "fixthreads.net", label = "Threads" },
}
local schema = {}
for _, r in ipairs(rules) do
  schema[#schema + 1] = { key = r.key, kind = "switch", default = r.key == "twitter",
    label = { ja = r.label .. " を変換する", en = "Convert " .. r.label, zh = "转换 " .. r.label } }
  schema[#schema + 1] = { key = r.key .. "_to", kind = "input", default = r.default,
    label = { ja = r.label .. " の変換先ドメイン", en = r.label .. " target domain", zh = r.label .. " 的目标域名" } }
end
local S = settings(schema)
local function convert(url)
  return (url:gsub("^(https?://)([^/]+)", function(scheme, host)
    local h = host:lower()
    for _, r in ipairs(rules) do
      if S.state[r.key] then
        for _, from in ipairs(r.hosts) do
          if h == from or h == "www." .. from or h == "mobile." .. from or h == "m." .. from or h == "vm." .. from or h == "old." .. from then
            local to = S.state[r.key .. "_to"]
            if to and to ~= "" then return scheme .. to end
          end
        end
      end
    end
    return scheme .. host
  end))
end
return plugin(S, {
  on_send = function(content)
    if not content:find("http", 1, true) then return nil end
    local out = content:gsub("https?://[^%s<>]+", convert)
    if out ~= content then return out end
  end,
})
