local S = settings({
  { key = "admins", kind = "switch", default = true, label = { ja = "管理者権限のある人にも印を付ける", en = "Also mark people with Administrator", zh = "也标记拥有管理员权限的人" } },
})
return plugin(S)
