local state = riscord.load() or {}
local default_targets = { en = "en", zh = "zh-CN" }
state.target = state.target or default_targets[riscord.language()] or "ja"
state.outgoing = state.outgoing or "en"
local function valid(value)
  return value and value:match("^[a-zA-Z][a-zA-Z%-]+$") and #value <= 10
end
return {
  translation_target = function() return state.target end,
  translation_outgoing = function() if state.auto_send then return state.outgoing end end,
  page = function()
    if riscord.language() == "en" then
      return {
        {kind="text", label="Text to translate is sent to Google Translate."},
        {kind="text", label="Incoming messages to: " .. state.target .. " / Outgoing messages to: " .. state.outgoing},
        {kind="switch", label="Auto-translate when sending", action="toggle_auto", enabled=not not state.auto_send},
        {kind="input", label="Language code for incoming messages (ja, en, fr, etc.)", action="target"},
        {kind="input", label="Language code for outgoing messages", action="outgoing"},
        {kind="button", label="Save languages", action="save"},
        {kind="text", label="Even with auto-translate off, you can type $ja hello. To skip auto-translate once, type !tr hello."},
      }
    end
    if riscord.language() == "zh" then
      return {
        {kind="text", label="要翻译的正文会发送到 Google 翻译。"},
        {kind="text", label="收到的消息译为：" .. state.target .. " / 发送的消息译为：" .. state.outgoing},
        {kind="switch", label="发送时自动翻译", action="toggle_auto", enabled=not not state.auto_send},
        {kind="input", label="收到消息的语言代码（zh-CN、en、ja 等）", action="target"},
        {kind="input", label="发送消息的语言代码", action="outgoing"},
        {kind="button", label="保存语言", action="save"},
        {kind="text", label="即使关闭自动翻译，也可以像 $en 你好 这样输入。若想临时不使用自动翻译，请输入 !tr 你好。"},
      }
    end
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
