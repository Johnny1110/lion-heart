# PRD 018: Pitch 家族 — Drop Tune 與 Octave

狀態：**草案（待開發）**
日期：2026-07-20
里程碑：M21（2026-07-20 路線圖第 8 項）
關聯：PRD 002（動態鏈）、PRD 007（新家族 + default_active）、PRD 017
（`MAX_SLOTS` 已升 16、DEFAULT_CHAIN 已 12）、白皮書 §5.4（自寫 DSP 深水區）

## 1. 背景與決策

競品招牌功能，**軟體圈幾乎空白**——論壇原話「Digitech has a lock on
polyphonic pitch-shifting plugins」。這是 Lion-Heart 最大的差異化機會，直接
服務 monster5150/angry-charlie 的金屬用戶（drop tuning 不換琴）。也是路線圖
最難的 DSP。

拍板：**新家族 `pitch`，兩踏板，放 DEFAULT_CHAIN 最前（gate 之前）**：

1. **位置最前**：真實 Drop/Whammy 踩在訊號鏈最前（音高先移、再進 amp）。
   DEFAULT_CHAIN 12→13（`pitch gate filter comp drive amp power eq mod delay
   reverb cab limiter`；`MAX_SLOTS` 已於 PRD 017 升 16）。**在 DEFAULT_CHAIN**
   而非 add-only，才能讓 plugin 使用者也用到。
2. **預設 bypass**（`default_active("pitch") = false`，同 filter/power）：
   移調從不透明，按亮才作用；LED 提示。
3. **兩踏板**（per-pedal `Ctl` 表）：
   - **drop**（Digitech Drop 式）：全音域多音移調，`shift` stepped
     −12..+12 半音（含升调 capo）；招牌「即時降弦」。
   - **octave**（POG/OC-2 式）：`sub`（下八度電平）+ `up`（上八度電平）+
     `dry` 混音——類比八度堆疊。
4. **延遲誠實**：多音移調有本質延遲（grain 視窗），v1 目標 Helix/QC 水準
   「撐得完一首歌」，不追無延遲。延遲納入未來 `docs/latency.md`。

## 2. 規格

**DSP（時域 granular/PSOLA 移調）**：延遲線 + **兩個重疊 grain**，讀取率
`2^(semitones/12)`，grain 邊界 Hann 交叉淡出藏接縫不連續。feedback-free、
輸出有界（RT 規則 7：無 runaway、無 NaN）。**mono-sum 偵測 + 移調**（Drop
本質單聲道；避免立體聲 grain 相位偽影），移調後複製雙聲道；`dry` 保留原
立體聲。

- **drop**：`shift`（半音）/ `mix`（乾/移調）/ `tone`（移調聲高頻補償）。
  下移 → grain 讀慢；上移 → 讀快。
- **octave**：`sub`（÷2 率）/ `up`（×2 率）/ `dry` / `tone`——三路混音，
  POG 式八度堆疊；上/下八度各一組 grain。

**品質基準（測試釘住）**：單音掃頻的移調輸出基頻落在 `f × 2^(n/12)`
（±cents 容差）；多音（和弦）移調不炸、諧波結構保留；grain 接縫無週期性
爆音（自相關無 grain-rate 尖峰）。

**Livery**：drop/octave 各 signature 色，納入 distinct-livery pin。plugin
自動展開 `pitch_drop_*`/`pitch_octave_*` + `pitch_pedal` + `pitch_active`
（預設 off）——**pre-v0.1 id 新增**，重跑 clap-validator。

## 3. 非目標

- 智慧和聲（音階感知 diatonic harmony）——這是固定音程移調，非 Harmonist。
- 表情踏板連續掃 Whammy（PRD 008 expression 架構可綁 `shift`，但 shift 是
  stepped；連續 whammy 掃頻 v2 另設連續 pitch 參數）。
- 相位聲碼器（phase vocoder）/ 頻域移調——v1 走時域 granular（延遲/CPU
  更可控）；若品質不足再評估 ADR。
- formant 保留 / 變性別人聲——吉他用不到。

## 4. 驗收標準

1. `cargo test`：移調基頻正確（掃頻各半音 ±cents）、octave sub/up 落在
   ±八度、和弦移調有界無 NaN、grain 接縫無週期爆音、mix/dry 0 邊界正確、
   bypass 位元透明、預設 bypass（default_active + plugin pin）、
   DEFAULT_CHAIN 13 槽 pin（三方）、多 rate/block、狂設定有界。
2. `cargo bench`：`pitch` 每踏板每 block 成本記入 `docs/benchmarks.md`
   （granular 較貴，可接受——設定實際數字上限）；延遲數字記入
   `docs/latency.md`。
3. `assert_no_alloc`：移調全程無配置（grain 緩衝 prepare 預配）。
4. 耳朵驗收（使用者）：drop −2 半音彈 drop-tuned riff（追蹤乾淨、金屬和弦
   不糊）、octave sub 加厚、up 加亮；延遲可接受（撐得完一首）；預設 bypass
   按亮才作用；plugin 內移調可用。
