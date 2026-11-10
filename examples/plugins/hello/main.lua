local state = riscord.load() or { count = 0 }

return {
  start = function() end,
  stop = function() riscord.save(state) end,
  on_message = function(message)
    state.count = state.count + 1
  end,
  on_send = function(content, channel_id)
    return content
  end,
  on_display = function(content, channel_id)
    return content
  end,
  commands = {
    hello = function(args, channel_id)
      riscord.send_message(channel_id, "こんにちは！ " .. args)
    end,
  },
  message_actions = {
    { label = "受信数を記録", run = function(message)
      riscord.save(state)
    end },
  },
  page = function()
    return {
      { kind = "text", label = "受信したメッセージ: " .. state.count },
      { kind = "input", label = "メモ", action = "memo" },
      { kind = "button", label = "メモを保存", action = "save" },
    }
  end,
  on_action = function(action, values)
    if action == "save" then
      state.memo = values.memo
      riscord.save(state)
    end
  end,
}
