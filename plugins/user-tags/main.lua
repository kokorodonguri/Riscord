local S = settings({
  { key = "tags", kind = "input", default = "", label = { ja = "タグ（ユーザーID=タグ1,タグ2; で区切る）", en = "Tags (userID=tag1,tag2; separated by ;)", zh = "标签（用户ID=标签1,标签2；用 ; 分隔）" } },
}, { ja = "ユーザー ID はメッセージにマウスを重ねたメニューや右クリックでコピーできます。", en = "Copy user IDs from the message menu or right-click.", zh = "可在消息菜单或右键菜单中复制用户 ID。" })
return plugin(S, {
  message_actions = {
    { label = "UserTags: " .. t({ ja = "この人の ID を入力欄へ", en = "Put this user's ID in the input", zh = "将此人的 ID 放入输入框" }), run = function(m) riscord.insert(m.author_id .. "=") end },
  },
})
