local S = settings({
  { key = "roles", kind = "input", default = "", label = { ja = "ロール（ロールID=名前|#rrggbb; で区切る。どちらか片方でも可）", en = "Roles (roleID=name|#rrggbb, separated by ;; either part is optional)", zh = "身份组（身份组ID=名称|#rrggbb，用 ; 分隔；可只写其一）" } },
}, { ja = "名前の色と TopRoleEverywhere の表示に反映します。", en = "Applies to name colors and TopRoleEverywhere.", zh = "应用于名称颜色和 TopRoleEverywhere 的显示。" })
return plugin(S)
