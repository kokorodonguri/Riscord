local S = settings({
  { key = "binds", kind = "input", default = "", label = { ja = "キーとプラグイン ID（ctrl+alt+1=plugin-id,plugin-id2; で区切る）", en = "Keys and plugin IDs (ctrl+alt+1=plugin-id,plugin-id2; separated by ;)", zh = "按键与插件 ID（ctrl+alt+1=plugin-id,plugin-id2；用 ; 分隔）" } },
}, {
  ja = "Ctrl・Alt・Super のどれかを含むキーにしてください。プラグイン ID は一覧の名前の下に出ています。",
  en = "Use keys with Ctrl, Alt or Super. Plugin IDs are shown under each name in the list.",
  zh = "请使用包含 Ctrl、Alt 或 Super 的按键。插件 ID 显示在列表中每个名称下方。",
})
return plugin(S)
