local S = settings({}, {
  ja = "/ts → 今。/ts 18:30 R → 今日（過ぎていれば明日）の 18:30 を相対表示。/ts 2026-12-01 20:00 F、/ts +2h のようにも書けます。表示の種類: t T d D f F R",
  en = "/ts → now. /ts 18:30 R → 18:30 today (tomorrow if past) as relative time. You can also write /ts 2026-12-01 20:00 F or /ts +2h. Styles: t T d D f F R",
  zh = "/ts → 现在。/ts 18:30 R → 今天（若已过则为明天）18:30 的相对时间。也可以写 /ts 2026-12-01 20:00 F 或 /ts +2h。样式：t T d D f F R",
})
return plugin(S, {
  commands = {
    ts = function(args)
      local at, rest = parse_when(args)
      if not at then
        at = riscord.now() // 1000
        rest = args
      end
      local st = trim(rest or ""):match("^([tTdDfFR])$") or "f"
      riscord.insert("<t:" .. at .. ":" .. st .. ">")
      return true
    end,
  },
})
