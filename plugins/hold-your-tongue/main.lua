local S = settings({
  { key = "keywords", kind = "input", default = "",
    label = { ja = "止める言葉（, 区切り）", en = "Words to stop (comma separated)", zh = "要拦截的词语（逗号分隔）" } },
  { key = "message", kind = "input", default = "",
    label = { ja = "止めたときの言葉（空なら既定）", en = "Message when stopped (empty: default)", zh = "拦截时显示的文字（为空则使用默认）" } },
}, {
  ja = "止められたメッセージは入力欄に残ります。15 秒以内にもう一度 Enter を押すと、そのまま送ります。",
  en = "A stopped message stays in the input. Press Enter again within 15 seconds to send it anyway.",
  zh = "被拦截的消息会留在输入框中。15 秒内再次按 Enter 即可照常发送。",
})
local last = nil
return plugin(S, {
  before_send = function(content)
    local lower = content:lower()
    for _, word in ipairs(split(S.state.keywords)) do
      if lower:find(word:lower(), 1, true) then
        if last and last.content == content and riscord.now() - last.at < 15000 then
          last = nil
          return nil
        end
        last = { content = content, at = riscord.now() }
        local msg = S.state.message
        if msg == nil or msg == "" then
          msg = t({ ja = "「" .. word .. "」を含んでいます。本当に送るならもう一度 Enter を押してください。",
                    en = "This contains \"" .. word .. "\". Press Enter again if you really want to send it.",
                    zh = "包含“" .. word .. "”。如果确定要发送，请再按一次 Enter。" })
        end
        return msg
      end
    end
  end,
})
