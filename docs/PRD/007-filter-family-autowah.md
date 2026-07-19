# PRD 007: Filter 家族 — Auto Wah(envelope filter)

狀態：**草案 → 開發中**
日期：2026-07-19
里程碑：M12
關聯：PRD 002（動態鏈）、PRD 006（mod 家族;auto-wah 歸屬的討論在此拍板）、白皮書 §4.2/§4.3

## 1. 背景與決策

使用者要 auto-wah。歸屬有兩案：塞進 mod 家族（零結構成本）或新開
`filter` 家族。**拍板：新開 `filter` 家族**——使用者明確預期之後會有
同類效果（LFO wah、sample & hold、formant filter），家族 key 取
`filter` 而非 `wah`，為成長留位。

新家族的結構代價與解法：

1. **DEFAULT_CHAIN 10 → 11 slots**（上限 12）。位置在 **gate 之後、
   comp 之前**——envelope follower 吃的是彈奏動態，而 compressor 的
   工作就是壓掉動態;放 comp 前面才有 touch response。
2. **濾波器沒有中性設定**（不像 gate/comp/limiter 預設透明），放上
   預設板會替所有人上色。新機制：`lh_core::default_active(family_key)`
   ——app 的預設板與 plugin 的 bypass 參數共用同一面旗，`filter`
   預設 **bypass**，亮燈由玩家決定。
3. 舊 preset（v1–v5）**零影響**：preset 定義結構，reconcile 會移除
   preset 裡沒有的 filter slot;無 schema 升版（家族是新增詞彙，
   forward-compat 規則處理舊版讀新檔）。

## 2. Auto Wah 規格

**Faceplate（6 鈕）**：`sens` / `q`（1.5–12 log）/ `decay`（60–600 ms
log）/ `mode`（lowpass/bandpass/highpass）/ `direction`（up/down）/
`mix`（預設 1.0 全 wet）。

**訊號路徑**：

1. **Envelope follower**：mono-sum 取 |x| × sens 前置增益（滿檔
   +30 dB，輕彈也推得滿掃程）→ 非對稱一階（attack 固定 2 ms——quack
   要咬在音頭上;release = `decay` 鈕——手感的靈魂）。
2. **掃頻**：env 幾何映射 180 Hz → 2.4 kHz（人耳聽頻率是對數的）;
   `direction` down 反轉（重擊往低掃）。
3. **濾波器**：Chamberlin SVF——一個結構同時給 LP/BP/HP（mode 鈕
   免費）、每 sample 重新調頻只要一個 `sin`;wah 頻段遠低於穩定上限
   （fc ≪ sr/6）。band 狀態每 sample 過 `tanh` 軟剪——Q 12 的共振像
   類比濾波器一樣飽和自限，永不失控（RT 規則 7）。
4. **立體聲**：envelope 讀 mono-sum、兩聲道共用一個掃頻（quack 是
   一個事件，同 vibrato 哲學）;SVF 狀態每聲道獨立。

**實測特性**（測試釘住）：Chamberlin BP 是 constant-skirt 型——Q 的
差異只出現在共振點（峰值增益 ≈ Q），裙帶與 Q 無關;性格測試在共振點
以小訊號（避開軟剪壓縮）量測。

## 3. 非目標

- 無 expression/MIDI CC 掃 wah（腳踏 wah 等 expression 架構）。
- 無 LFO wah / S&H——它們是這個家族的下一批 pedal，不是這一批。
- filter 家族不進 mod 的 Ctl 表——它是獨立單 pedal 家族，自己的
  set_param 路由。

## 4. 驗收標準

1. `cargo test`：envelope 追彈奏強度（大聲開、小聲關）、direction
   down 反轉、三 mode 相異、高 Q 共振增益、最大設定有界、mix 0
   bit-exact、多取樣率、旋鈕掃掠不炸;DEFAULT_CHAIN/registry/plugin
   三方 pin 測試含 **filter 預設 bypass**（app 與 plugin 同一面旗）。
2. `cargo bench`：`filter_autowah` < 0.15 % deadline。
3. 舊 preset 載入結構不變（無 filter slot）;新預設板 filter 燈滅。
4. 耳朵驗收（使用者）：clean funk 輕重交替聽 quack 追手;direction
   down + 高 q 聽反向怪聲;拖到 drive 後面聽合成器味;bypass 燈
   預設滅、按亮才有作用。
