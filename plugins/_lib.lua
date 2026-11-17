-- 同梱プラグイン共通の部品。各同梱プラグインの main.lua の前に連結して読み込む（require は使えないため）

-- {ja=, en=, zh=} から表示言語の文字列を選ぶ
local function t(m)
  if type(m) ~= "table" then return m end
  return m[riscord.language()] or m.ja or m.en or ""
end

local function trim(s)
  return (s:gsub("^%s+", ""):gsub("%s+$", ""))
end

-- sep（1 文字の区切り。既定は ,）で分けて、空でない項目を前後の空白を除いて返す
local function split(s, sep)
  local out = {}
  if type(s) ~= "string" then return out end
  sep = sep or ","
  for part in (s .. sep):gmatch("(.-)" .. sep:gsub("%p", "%%%0")) do
    part = trim(part)
    if part ~= "" then out[#out + 1] = part end
  end
  return out
end

-- "a=b; c=d" を {a="b", c="d"} にする
local function pairs_of(s, sep)
  local out = {}
  for _, item in ipairs(split(s, sep or ";")) do
    local k, v = item:match("^(.-)=(.*)$")
    if k then out[trim(k)] = trim(v) end
  end
  return out
end

-- 宣言的な設定。schema の各項目は
--   {key=, kind="switch"|"input"|"number", label={ja=,en=,zh=}, default=}
--   {kind="text", label=...} / {kind="button", action=, label=...}
-- 戻り値の page / on_action / options をプラグイン表にそのまま入れる。
-- 設定は riscord.save で保存し、options() で本体からも読める。
-- P.handle(action, values) を置くと、ボタンなど設定以外の操作を受け取れる。
-- P.extra_page() を置くと、ページの末尾に部品を足せる。
local function settings(schema, intro)
  local state = riscord.load() or {}
  for _, s in ipairs(schema) do
    if s.key and state[s.key] == nil then state[s.key] = s.default end
  end
  local P = { state = state }
  P.page = function()
    local items = {}
    if intro then items[#items + 1] = { kind = "text", label = t(intro) } end
    local has_input = false
    for _, s in ipairs(schema) do
      local label = t(s.label)
      if s.kind == "switch" then
        items[#items + 1] = { kind = "switch", label = label, action = s.key, enabled = not not state[s.key] }
      elseif s.kind == "text" then
        items[#items + 1] = { kind = "text", label = label }
      elseif s.kind == "button" then
        items[#items + 1] = { kind = "button", label = label, action = s.action or s.key }
      else
        has_input = true
        local v = state[s.key]
        if v == nil then v = "" end
        local now = t({ ja = "現在: ", en = "current: ", zh = "当前：" })
        items[#items + 1] = { kind = "input", label = label .. "（" .. now .. tostring(v) .. "）", action = s.key }
      end
    end
    if has_input then
      items[#items + 1] = { kind = "button", label = t({ ja = "設定を保存", en = "Save settings", zh = "保存设置" }), action = "__save" }
    end
    if P.extra_page then
      for _, it in ipairs(P.extra_page()) do items[#items + 1] = it end
    end
    return items
  end
  P.on_action = function(action, values)
    if action == "__save" then
      for _, s in ipairs(schema) do
        local v = values[s.key]
        if (s.kind == "input" or s.kind == "number") and v ~= nil and v ~= "" then
          if s.kind == "number" then
            local n = tonumber(v)
            if n then state[s.key] = n end
          elseif v:match("^%s+$") then
            state[s.key] = ""
          else
            state[s.key] = trim(v)
          end
        end
      end
      riscord.save(state)
      if P.changed then P.changed() end
      return
    end
    for _, s in ipairs(schema) do
      if s.kind == "switch" and s.key == action then
        state[s.key] = not state[s.key]
        riscord.save(state)
        if P.changed then P.changed() end
        return
      end
    end
    if P.handle then P.handle(action, values) end
  end
  P.options = function() return state end
  return P
end

-- settings の結果（なければ nil）にフックを足したプラグイン表を返す
local function plugin(base, hooks)
  local p = {}
  for k, v in pairs(base or {}) do
    if k ~= "state" and k ~= "handle" and k ~= "extra_page" and k ~= "changed" then p[k] = v end
  end
  for k, v in pairs(hooks or {}) do p[k] = v end
  return p
end

