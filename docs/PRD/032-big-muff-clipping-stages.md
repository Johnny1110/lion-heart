# PRD 032: `big-muff` — 兩級回授二極體削波 + Muff tone stack（Phase 05 第一顆）

狀態：**已實作（2026-07-29）— 待使用者耳朵驗收**
日期：2026-07-29
里程碑：Tone Revolution · Phase 05（`docs/tone_revolution/phase/05-fuzz-transistor-family.md` §2.1）
關聯：**ADR 035**（電晶體建模法）、PRD 021 / ADR 029（`sd1`，同機制類）、
PRD 023 / ADR 030（tone stack 引擎，本顆第一次真正用上 `big-muff` 那組 netlist）
新增 ADR：**ADR 035**（與 PRD 033 / 034 共用）

## 1. 背景與決策

Phase 04 六顆全是 op-amp + WDF。這一顆是這條線的第一個**例外**，而例外的理由要寫清楚：
Big Muff 的削波級是「共射級 + 回授路徑上的反並聯二極體」，**削波機制與 `sd1` 完全同類**
（二極體削的是回授阻抗，不是訊號），但那顆「op-amp」是被線性化成 `A = −Rc/Re` 的電晶體
——**它沒有可宣稱的輸入/輸出阻抗**，而 R-Type junction 正需要那兩個數字。ADR 033 才剛把
「op-amp 參數來自 datasheet、不准編造」寫成政策，這裡不能自打嘴巴。

所以走節點方程 + 標量阻尼 Newton（`blocks::transistor::ShuntFeedbackStage`，ADR 035 §1）。
**不需要任何新的求解機制**——這是計畫說它「最有價值也最可行」的原因，實作後確認。

### 1.1 對參考實作的一處修正：戴維南電阻用交流值

細節與量化在 **ADR 035 §3.1**。摘要：BYOD 把回授電流注入在 `R20`（100 kΩ，該網路的
**直流**戴維南電阻）上，但它的輸入濾波器係數同時證明那是一個「源 —C5— R19— 節點，
R20 對交流地」的網路，其**交流**戴維南電阻是 `R19 ‖ R20 = 9.09 kΩ`。

差別是每一級 **−4.3× vs −28.0×**，6.5 倍。本專案用交流值，並由 §3.1 的手解 AC 分析
釘住（那條測試與實作零共用推理）。

## 2. 規格

### 2.1 一級的 netlist（元件值查自 schematic）

```
源 —C5(100n)— R19(10k) — 和節點 —— R20(100k) — 交流地
和節點 → 共射級 A = −Rc/Re = −10k/150 = −66.7 → 輸出 y
回授（y → 和節點）：R17(470k) ‖ C12(470p) ‖ 反並聯 1N4148 對
```

放大器把和節點釘在 `y/A`，所以回授網路兩端是 `κ·y`（`κ = 1 − 1/A = 1.015`），KCL 收成
一條標量方程（ADR 035 §1）。`C12` 走梯形（雙線性）伴隨模型。

派生常數，全部由上面四個電阻算出、不另外調：

| 常數 | 值 | 意義 |
| ---- | -- | ---- |
| `R_TH = R19‖R20` | 9.09 kΩ | 回授電流注入的戴維南電阻（§1.1） |
| `IN_DIV = R20/(R19+R20)` | 0.909 | 同網路的開路分壓 |
| `F_IN = 1/(2π·C5·(R19+R20))` | 14.5 Hz | 同網路的高通轉角 |

二極體：1N4148，`(Is, n) = (2.52e-9, 1.75)`（ADR 033 的兩參數慣例，與 `sd1`/`screamer`
同一組）。

### 2.2 兩級串接，然後是 tone stack

真實 Big Muff 是 Q1 升壓 → Q2 削波 → Q3 削波 → tone stack → Q4 回復。本模型：

```
Sustain（輸入增益，Q1 與 Sustain 電位器合併）
  → 削波級 ×2（各自帶自己的輸入網路，這就是級間耦合）
  → eq::tonestack 的 big-muff 模型（Phase 02）
  → makeup（Q4 的位置）+ DC block
```

兩級都反相，所以整體同相。**Tone stack 是 Phase 02 的直接回報**：Muff 的 tone 不是
tilt 而是**兩條路徑相加造成的凹陷**，凹陷位置隨旋鈕滑動——那組 netlist 從 PRD 023 起就
躺在 `eq::tonestack` 裡，這顆是第一個真正用它的踏板。

### 2.3 三處刻意的簡化

- **不建直流偏壓**（延續 ADR 034 §4）。BYOD 用 `VbiasA = 0.7` 當二極體的參考點，代價是
  輸出帶一個要靠 DC blocker 擋掉的假直流。本模型以 0 V 為參考，二極體對稱削波——真實
  Muff 本來就以對稱方波著稱——換得 **`y = 0` 是節點方程的精確不動點**，silence in →
  silence out 是恆等式而不是「夠小」。
- **兩級元件值相同**。真實 Muff 歷代（Triangle / Ram's Head / Sovtek）兩級略有差異；
  先做一版代表值，變體留給日後的 stepped 選擇器（`ShuntFeedbackStage::new` 已經收
  全部元件值，加選單即可）。
- **沒有 Smoothing 旋鈕**。參考實作有一個 ±200 pF 掃 `C12` 的控制，那是它的發明不是
  Muff 的面板。面板照硬體：**Sustain / Tone / Volume**。

### 2.4 Sustain 的範圍

一級在 ~14 mV 輸入就開始削（0.4 V 膝部 ÷ 28.0 增益）。所以 Sustain 掃
`−20 dB … +34 dB`（`(pos/10)^1.5` taper）：**0 就坐在膝部上**（Muff 從來不乾淨），
5 約略 unity，10 把兩級推爆三個數量級。

## 3. 驗收標準與實測

### 3.1 `cargo test`（lh-dsp 441 → **449**，workspace 全綠，debug 與 release 皆綠）

`blocks::transistor` 另有 11 條（求解器契約、Ebers–Moll 的 Jacobian 對數值微分、3×3 解）。

| 測試 | 標準 | 實測 |
| ---- | ---- | ---- |
| `the_stage_solves_its_node_equation`（blocks） | 9 個 `u`（±1e3 到 ±1e-4）殘差代回 | 全部 < 1e-9 V |
| `the_small_signal_gain_matches_the_textbook_inverting_amplifier`（blocks） | 收斂後閉迴路增益 vs 教科書有限增益反相放大器 | < 2 % |
| **`the_linear_response_matches_hand_solved_ac_analysis`** | 6 個頻率，兩級串接 vs 手解 AC 分析（輸入高通 × 有限增益級，雙線性預扭） | 全部 < 3 % |
| `the_cascade_gain_is_what_the_components_say` | 兩級合計落在 600–700× | 通過（~650× = 56 dB） |
| `the_smoothing_cap_makes_break_up_frequency_dependent` | 同振幅下 120 Hz 諧波 > 4 kHz 的 1.2 倍 | 通過 |
| `the_second_stage_is_what_makes_it_a_wall` | 輸入退 12 dB 後電平比 > 0.85（線性是 0.25） | 通過 |
| `core_silence_in_silence_out` | **精確** 0 | 通過（`assert_eq!`） |
| `core_bounded_when_slammed` | ±1e6 交替、冷啟動 | 有限且 < 2 V |

**AC 分析那條是本顆的關鍵測試**，有兩個理由：它是 §1.1 那個 6.5 倍修正的唯一裁判；
而且它必須把**二極體的零偏阻抗**算進去——反並聯對在 0 V 附近有 `nVt/2Is` = 9 MΩ 的
有限電阻，跨在 470 kΩ 上要吃掉 3 % 增益。漏掉它就是「手算與模型差幾個百分點」的
經典來源，本次也確實先漏了一次。

**白箱判別 = `C12`。** 它跨在 470 kΩ 上，轉角 720 Hz：**二極體沒導通時**這一級是
6 dB/oct 低通；**導通後**回授阻抗塌到幾十歐姆，轉角跑到 MHz。所以這顆踏板
「濾掉它不削的、放行它削的」——Muff 平順不刺的高頻來自這一顆電容，而 memoryless
曲線兩個方向都做不到。

### 3.2 `cargo bench`（同一輪，全家族）

| Bench | 中位數 | ÷ screamer |
| ----- | ------ | ---------- |
| `drive_screamer_4x_oversampled` | ~35.9 µs | 1.00（校準） |
| `drive_king-of-tone_4x_oversampled` | ~47.6 µs | 1.33 |
| **`drive_big-muff_4x_oversampled`** | **~85.3 µs** | **2.38** |

2.38× 是**兩次 Newton 每 sample** 的價（`king-of-tone` 是兩個 WDF root，便宜一些：
root 的閉式解不迭代）。85 µs = 6.4 % of the 64-frame deadline。

### 3.3 電平與抗鋸齒

預設旋鈕 **−0.01 dB**（`MAKEUP = 0.195`）。alias floor **−34.4 dB**，釘在 −30。
偏高但不是家族最高（`rangemaster` −24、`rat` −30）——`C12` 的迴路低通實際上幫了忙，
比「先放大 200 倍再削平」的 `mxr-dist`（−34.6）持平。ADAA 在這裡**不適用**：非線性是
解出來的電路，不是顯式曲線（PRD 024 的前提）。

### 3.4 耳朵（**待使用者驗收**）

- Sustain 掃全程：0 應該在崩壞邊緣、10 是牆。
- Tone 掃過中頻凹陷（「反 wah」），聽凹陷位置滑動而不是高低頻蹺蹺板。
- 與 `sd1` 對照最有意思：**同一類機制**（回授二極體），一級 vs 兩級、op-amp vs 電晶體。

## 4. 非目標

- 不建 9 V 電源軌削頂、不建直流偏壓（§2.3）。
- 不做年代變體。
- 不追 alias floor——兩級硬削 + 無 ADAA 就是這個地板。
- 不做 Smoothing 旋鈕（參考實作的發明，不是 Muff 的面板）。

## 5. 已知取捨

- **共射級是一個增益常數**：電晶體自己的非線性、`re` 隨電流變化都沒有。Muff 的削波
  幾乎全由二極體決定，所以這個簡化大致無害，但推到極端會少一層層次。
- **`A = −Rc/Re = −66.7` 是理想值**。真實共射級是 `−Rc/(Re + re′)`，`re′ = Vt/Ie` 在
  幾百 µA 時是 60–70 Ω，對 150 Ω 不可忽略——實機增益會低一些。沿用參考值以保持與
  公開電路分析可對照。
- **alias floor −34.4 dB**（§3.3）。

## 6. 產出

- `crates/lh-dsp/src/blocks/transistor.rs`（新模組，與 PRD 033 共用）
- `crates/lh-dsp/src/drive/big_muff.rs`（新）
- registry / `DRIVE_PEDALS` / theme livery 追加
- `docs/adr/035-transistor-modelling.md`、`docs/benchmarks.md`
