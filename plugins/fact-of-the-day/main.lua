local S = settings({
  { key = "every_start", kind = "switch", default = false, label = { ja = "起動するたびに出す", en = "Show on every start", zh = "每次启动都显示" } },
  { kind = "button", action = "now", label = { ja = "今すぐ 1 つ見る", en = "Show one now", zh = "现在看一条" } },
})
local facts = {
  { ja = "タコの心臓は 3 つあります。", en = "Octopuses have three hearts.", zh = "章鱼有三颗心脏。" },
  { ja = "ハチミツは何千年たっても腐りにくい食べ物です。", en = "Honey can stay edible for thousands of years.", zh = "蜂蜜可以保存数千年都不容易变质。" },
  { ja = "バナナは植物学的にはベリーの仲間です。", en = "Botanically, bananas are berries.", zh = "从植物学角度看，香蕉属于浆果。" },
  { ja = "金星の 1 日は 1 年より長いです。", en = "A day on Venus is longer than its year.", zh = "金星上的一天比它的一年还长。" },
  { ja = "ラッコは眠るとき、流されないよう手をつなぐことがあります。", en = "Sea otters sometimes hold hands while sleeping so they don't drift apart.", zh = "海獭睡觉时有时会牵手，以免被冲散。" },
  { ja = "エッフェル塔は夏の暑さで 15 cm ほど高くなります。", en = "The Eiffel Tower can grow about 15 cm taller in summer heat.", zh = "埃菲尔铁塔在夏天受热时会长高约 15 厘米。" },
  { ja = "カタツムリは 3 年近く眠り続けることがあります。", en = "Snails can sleep for up to about three years.", zh = "蜗牛最长可以睡将近三年。" },
  { ja = "人の骨のうち、半分以上は手と足にあります。", en = "More than half of the bones in the human body are in the hands and feet.", zh = "人体一半以上的骨头位于手和脚。" },
  { ja = "キリンの舌は 50 cm ほどあります。", en = "A giraffe's tongue can be about 50 cm long.", zh = "长颈鹿的舌头长约 50 厘米。" },
  { ja = "光が太陽から地球に届くまで約 8 分 20 秒かかります。", en = "Sunlight takes about 8 minutes 20 seconds to reach Earth.", zh = "阳光从太阳到达地球大约需要 8 分 20 秒。" },
  { ja = "ペンギンにも膝があります。", en = "Penguins have knees.", zh = "企鹅也有膝盖。" },
  { ja = "最初のコンピューターのバグは、本物の蛾でした（1947 年）。", en = "The first computer \"bug\" was an actual moth (1947).", zh = "第一个计算机“bug”是一只真的飞蛾（1947 年）。" },
}
local function show()
  local i = (riscord.now() // 1000) % #facts + 1
  riscord.notify(t({ ja = "今日の豆知識", en = "Fact of the day", zh = "今日小知识" }), t(facts[i]))
end
S.handle = function(action) if action == "now" then show() end end
return plugin(S, {
  start = function()
    local day = riscord.date("%Y-%m-%d")
    if S.state.every_start or S.state.last_day ~= day then
      S.state.last_day = day
      riscord.save(S.state)
      riscord.set_timeout(3000, show)
    end
  end,
})
