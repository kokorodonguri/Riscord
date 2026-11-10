local state = riscord.load() or {}
state.target = state.target or "ja"
state.outgoing = state.outgoing or "en"
local function valid(value)
  return value and value:match("^[a-zA-Z][a-zA-Z%-]+$") and #value <= 10
end
return {
  translation_target = function() return state.target end,
  translation_outgoing = function() if state.auto_send then return state.outgoing end end,
  page = function()
    return {
      {kind="text", label="翻訳対象の本文は Google 翻訳に送信されます。"},
      {kind="text", label="受信文の翻訳先: " .. state.target .. " / 送信文の翻訳先: " .. state.outgoing},
      {kind="switch", label="送信時の自動翻訳", action="toggle_auto", enabled=not not state.auto_send},
      {kind="input", label="受信文の言語コード（ja、en、fr など）", action="target"},
      {kind="input", label="送信文の言語コード", action="outgoing"},
      {kind="button", label="言語を保存", action="save"},
      {kind="text", label="自動翻訳がオフでも $en こんにちは のように入力できます。自動翻訳を一時的に使わない場合は !tr こんにちは と入力します。"},
    }
  end,
  on_action = function(action, values)
    if action == "save" then
      if valid(values.target) then state.target = values.target end
      if valid(values.outgoing) then state.outgoing = values.outgoing end
      riscord.save(state)
    elseif action == "toggle_auto" then
      state.auto_send = not state.auto_send
      riscord.save(state)
    end
  end,
}
