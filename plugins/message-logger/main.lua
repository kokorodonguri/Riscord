-- 削除・編集されたメッセージを残す。記録と表示は本体が行い、ここでは記録の条件を持つ
local state = riscord.load() or {}
if state.ignore_self == nil then state.ignore_self = true end
state.ignore_bots = not not state.ignore_bots
return {
  logger_options = function()
    return { ignore_self = state.ignore_self, ignore_bots = state.ignore_bots }
  end,
  page = function()
    if riscord.language() == "en" then
      return {
        {kind="text", label="Keeps deleted messages in red and shows the text before edits below each message. The log is saved only in this PC's settings folder (message-log.json)."},
        {kind="switch", label="Don't log my own messages", action="ignore_self", enabled=state.ignore_self},
        {kind="switch", label="Don't log bot messages", action="ignore_bots", enabled=state.ignore_bots},
        {kind="button", label="Clear the whole log", action="clear"},
      }
    end
    if riscord.language() == "zh" then
      return {
        {kind="text", label="以红色保留被删除的消息，并在每条消息下方显示编辑前的正文。记录只保存在这台电脑的设置文件夹（message-log.json）中。"},
        {kind="switch", label="不记录自己的消息", action="ignore_self", enabled=state.ignore_self},
        {kind="switch", label="不记录机器人的消息", action="ignore_bots", enabled=state.ignore_bots},
        {kind="button", label="清除全部记录", action="clear"},
      }
    end
    return {
      {kind="text", label="削除されたメッセージを赤く残し、編集前の本文をメッセージの下に表示します。記録はこのパソコンの設定フォルダ（message-log.json）にだけ保存されます。"},
      {kind="switch", label="自分のメッセージは記録しない", action="ignore_self", enabled=state.ignore_self},
      {kind="switch", label="Bot のメッセージは記録しない", action="ignore_bots", enabled=state.ignore_bots},
      {kind="button", label="記録をすべて消す", action="clear"},
    }
  end,
  on_action = function(action)
    if action == "ignore_self" then
      state.ignore_self = not state.ignore_self
      riscord.save(state)
    elseif action == "ignore_bots" then
      state.ignore_bots = not state.ignore_bots
      riscord.save(state)
    end
  end,
}
