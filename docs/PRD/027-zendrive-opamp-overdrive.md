# PRD 027: `zendrive` — 透明系 op-amp overdrive（Phase 04 第二顆）

狀態：**已實作（2026-07-28）— 待使用者耳朵驗收**
日期：2026-07-28
里程碑：Tone Revolution · Phase 04（`docs/tone_revolution/phase/04-opamp-overdrive-family.md` §2.2）
關聯：PRD 026 / ADR 033（`ts-wdf` 與家族參數政策）、PRD 025 / ADR 032（WDF 框架）
新增 ADR：**034（削波器自行擬合 + 對計畫判讀的修正）**

## 1. 背景與決策

這顆是**框架紅利的第一次兌現**。它的 netlist 形狀與 `ts-wdf` **完全相同**——同樣的
節點、同樣四個 port、同樣的 `NON_INVERTING_PORTS` 佈局——所以移植它是「選零件」，
不是「推電路」。兩顆都沒有任何地方寫著散射矩陣：各自從共用拓撲 + 各自的 op-amp
在旋鈕率數值構造（ADR 032）。

本次把那個共用佈局提到框架層（`blocks::wdf::{NON_INVERTING_NODES,
NON_INVERTING_PORTS, non_inverting_els}`），`ts-wdf` 一併改用。這讓「TS 與 ZenDrive
共用一個散射矩陣」這件事變成**結構上的事實**，而不是註解裡的一句話——而且
`mxr-dist` 接下來直接沿用。

| | Tube Screamer | ZenDrive |
| --- | --- | --- |
| gain leg | 固定 `4.7k + 47n` | **Voice 鈕**，`1k…11k + 100n` |
| 回授 | `51k…551k ‖ 51p` | `1k…500k ‖ 100p` |
| 輸入 | `1µ` 進 `10k` | `470n` 進 `470k` |
| 削波 | 矽二極體，~0.6 V | **MOSFET 疊層，~1.0 V** |

**為什麼透明**，兩個原因都在表裡：(1) 削波膝部比 Screamer 高將近一伏，吉他電平下
這一級大多在**放大**而不是削波，破音是漸進的；(2) gain leg 的電容是 100 nF（TS 是
47 nF）配上五分之一的電阻，低頻轉角落在 **145 Hz–1.6 kHz** 而非固定 720 Hz——低頻
是穿過去的，**沒有 mid-hump**。那就是這顆踏板的全部重點。

**Voice 鈕就是那個轉角**：轉大 → gain leg 降到 1 kΩ → 增益上升、轉角爬到 1.6 kHz
（中高頻推出來）；轉小 → 11 kΩ，幾乎平、幾乎乾淨。它**在 R-Type 的 port 裡**，所以
動它會重算散射矩陣——在子區塊邊界、有 glide、絕不逐 sample。

## 2. 規格

### 2.1 Netlist

共用 `NON_INVERTING_PORTS`：
`[ (out,−) up=回授(含削波器), (+,gnd) 輸入腳, (−,gnd) gain leg, (out,gnd) 負載 ]`

元件：`C3=470n`、`R4=470k`、`R5=1k`、`R6=10k`（Voice）、`C5=100n`、`R9=500k`
（Gain）、`C4=100p`、`RL=1M`。

```rust
type InputLeg  = Parallel<CapacitiveVoltageSource, Resistor>;
type OpAmpNode = RType<4, 3, (InputLeg, ResistorCapacitorSeries, Resistor)>;
type ClipTree  = Parallel<ResistorCapacitorParallel, OpAmpNode>;   // 與 ts-wdf 同型別
```

op-amp（ADR 033 政策）：schematic 未標型號 → 取同級典型值並標明為**推定**——
TL072，`AG=3000`（3 MHz GBW @ 1 kHz）、`RI=1e9`（JFET 輸入）、`RO=200`。

旋鈕律：`Rf = 500k·(0.002 + 0.998·n²)`（audio taper，底限不是裝飾——0 會讓回授短路）；
`R_voice = 1k + 10k·(1 − n)`（**反向**：Voice 轉大是把電阻拿掉）。

### 2.2 削波器（詳見 ADR 034）

反並聯兩支，每支 = **1N4148 串一顆二極體接法的 2N7002 MOSFET**（拓撲事實）。
兩顆元件疊在一起，所以膝部高、斜率緩，**單一矽二極體的 `Is`/`n` 描述不了它**。

`IS = 7.50e-11`、`THERMAL_V = 0.0729 V`，**本專案自行擬合**該削波器的 SPICE 曲線，
取電路實際工作的 1 µA–300 µA；四個對照點誤差 −8 / +15 / +7 / −4 mV。

**兩處對參考實作的修正**：

1. **接線 bug 修掉**：參考實作把二極體的阻抗參考掛在輸入級 port（~5 Ω），波卻與
   回授節點（~20 kΩ）交換——`a = v + R·i(v)` 裡的 `R` 差 3,600 倍。我們面對回授節點。
2. **擬合值不沿用**，但**理由與計畫寫的不同**（ADR 034 §1）：計畫 v2 深審推測那組
   值是「繞著 bug 擬出來的」，且 `nVt` 是物理 `Vt` 的 3 倍即為證據。查證後**兩點都
   不成立**——擬合是對著一份獨立的 LTspice 削波器電路離線做的，bug 不可能參與；
   `nVt` 反常單純因為每支是**兩顆元件串聯**。不沿用的真正理由是可量測的：它擬的是
   `Is·sinh`、跑的是 `2·Is·sinh`，整個工作範圍低 **60–105 mV**（≈ `nVt·ln 2`）。

**4.5 V 偏壓不建模**（ADR 034 §4）：我們的 op-amp 沒有電源軌，那條偏壓在訊號路徑
上是純共模，只換來一段開機暫態（參考實作得空跑 20,000 sample 等它穩）。

### 2.3 面板：Gain / Voice / Tone / Level

Voice 走 `Ctl::Trim`（PRD 026 建的 hook），在電路內以 ~12 ms 一階 glide 走——它動的
是 port 阻抗，一步跳過去就是增益與頻響同時跳。Tone 是掃頻低通 900 Hz–12 kHz，
**沒有 tilt、沒有 mid-hump**：這顆的後級要儘量不出聲。

## 3. 驗收標準與實測

### 3.1 `cargo test`（lh-dsp 378 → **389**，workspace 全綠，debug 與 release 皆綠）

| 測試 | 標準 | 實測 |
| ---- | ---- | ---- |
| `the_linear_response_matches_hand_solved_ac_analysis` | 對照手解 AC 分析，**3 voice × 3 gain × 4 頻率** | 36 組全數 < 2 % |
| `the_clipper_matches_the_fitted_device_curve` | 對照器件曲線 4 點 < 20 mV；且膝部比矽高 ≥ 0.35 V、每十倍電流斜率緩 ≥ 1.3× | 通過（誤差 ≤ 15 mV） |
| `the_clipper_root_is_solved_to_tolerance` | `a = v + R·i(v)`，50 000 sample 掃幅掃頻 | 最差相對殘差 < 1e-3 |
| `voice_sweeps_the_bass_corner_and_the_gain` | Voice 轉大：低/中頻比降到 < 0.6×，且中頻增益 > 1.5× | 通過 |
| `it_stays_cleaner_than_the_screamer_at_matched_settings` | 同旋鈕同輸入下諧波 < 0.5× `ts-wdf` | 通過（0.066 vs 0.198） |
| `it_cleans_up_far_better_than_the_screamer` | 輸入退 12 dB 後諧波 < 0.4×；且**保留比**是 `ts-wdf` 的 < 0.5× | 通過（保留 0.27 vs 0.80） |
| `its_level_tracks_the_input_where_the_screamer_compresses` | 同樣 12 dB 退檔的電平比 < 0.7× `ts-wdf` | 通過（0.32 vs 0.60；完全線性是 0.25） |
| `the_voice_knob_glides_instead_of_stepping` | 會動、一個 block 內到不了、最終到位 | 通過 |
| `the_response_holds_across_sample_rates` | 44.1/48/96 kHz 對同一份 AC 分析 | 全數 < 2 % |
| `silence_stays_silent` / `bounded_when_slammed` | 靜音出精確 0；±1e6 狂推（兩鈕四角）有界有限 | 通過 |

**character pin 是比較式的**，這是刻意的：任何削波器退檔都會乾淨一些，絕對數字說明
不了什麼。有辨識度的是**它比 Screamer 多走多遠**——退 12 dB 後 ZenDrive 只留 27 % 的
諧波，Screamer 留 80 %。那個差距直接來自高而緩的 MOSFET 膝部：訊號是**走出去**的，
不是坐在上面。

### 3.2 `cargo bench`（同一輪）

| Bench | 中位數 |
| ----- | ------ |
| `drive_screamer_4x_oversampled` | ~31.4 µs |
| `drive_ts-wdf_4x_oversampled` | ~40.3 µs |
| **`drive_zendrive_4x_oversampled`** | **~41.2 µs** |
| `drive_sd1_4x_oversampled` | ~69.2 µs |

41.2 µs ＝預算 3.1 %，與 `ts-wdf` 幾乎相同——同一個 junction、同樣的每-sample 工作量。

### 3.3 電平與抗鋸齒

預設旋鈕 **+0.55 dB**（`MAKEUP = 0.123`），家族中位附近。alias floor −44.6 dB，
釘在 −40（WDF 踏板不走 ADAA，此列為參照）。

### 3.4 耳朵（**待使用者驗收**）

與 `jan-ray`（Timmy 系，同屬透明派）A/B，以及與 `ts-wdf` 的對照：吉他音量轉小時
是否真的乾淨掉、Voice 由 0 掃到 10 的中頻推進、低頻是否比 TS 系飽滿。

## 4. 非目標

- 不建 4.5 V 偏壓／電源軌削頂（ADR 034 §4）。
- 不做器件級 MOSFET root（平方律／次臨界）——`DiodePair` 是指數對，疊層以等效斜率
  表示；器件級是 Phase 08 的題目。
- 不動任何既有踏板。

## 5. 已知取捨

- **`THERMAL_V` 是疊層的等效熱電壓**，不是任何單一元件的 ideality；程式碼以
  `THERMAL_V / VT` 餵給 `DiodePair` 的 `n` 參數，需靠註解避免誤讀。
- 擬合只在 **1 µA–300 µA** 有保證（1 mA 處仍在 4 mV 內，再高沒驗過）。
- **op-amp 型號是推定的**（schematic 未標），依 ADR 033 取同級典型值。

## 6. 產出

- `crates/lh-dsp/src/drive/zendrive.rs`（新）
- `crates/lh-dsp/src/blocks/wdf/rtype.rs`：`NON_INVERTING_NODES` /
  `NON_INVERTING_PORTS` / `non_inverting_els`——家族共用 junction 提到框架層
- `crates/lh-dsp/src/drive/ts_wdf.rs`：改用共用佈局（無行為變化）
- `crates/lh-dsp/tests/alloc.rs`：**修正**——`assert_no_alloc` 在 release 被
  feature-gate 掉，原本的寫法會讓 `cargo test --release` 編不過（PRD 026 的疏漏，
  當時只跑了 debug 閘門）。現在 allocator 與守衛都 `#[cfg(debug_assertions)]`，
  release 仍跑掃描並檢查有限性
- registry / `DRIVE_PEDALS` / theme livery 追加；`docs/adr/034-zendrive-clipper-fit.md`、
  `docs/benchmarks.md`
