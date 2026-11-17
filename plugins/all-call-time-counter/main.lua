local S = settings({}, {
  ja = "Riscord が入室を見た時刻から数えます。起動したときすでに入っていた人は、起動した時刻から数えます。",
  en = "Counts from when Riscord saw the person join. People already in a channel at startup are counted from startup.",
  zh = "从 Riscord 看到对方加入时开始计时。启动时已在频道中的人从启动时开始计时。",
})
return plugin(S)
