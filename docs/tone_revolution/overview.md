# Tone Revolution — 移植計畫藍圖（Overview）

狀態：**規劃中（draft）— 待使用者拍板分期與範圍**
日期：2026-07-24
里程碑：白皮書 §6 深水區研究線（WDF 白箱電路模擬）之總攻計畫；承接
PRD 020 / ADR 028（WDF TS 削波級）、PRD 021 / ADR 029（WDF 回授式 overdrive
`sd1`）
研究來源：`/mnt/BYOD`（Build-Your-Own-Distortion，ChowDSP，GPL-3）、
`/mnt/chowdsp_wdf`（Wave Digital Filter 函式庫，ChowDSP，BSD-3）——兩者同一作者
Jatin Chowdhury；`chowdsp_wdf` 正是 BYOD 的底層，也是 lion-heart `blocks::wdf`
手工重建之物的**成熟上游參考**。

---

## 0. 一句話

把 lion-heart 目前「一顆一顆手寫、以 memoryless 波形整形為主」的 drive/tone，
升級成一套**可組合的白箱電路框架**——讓（1）tone stack 是真實被動網路而非圖形
EQ、（2）業界名踏板的既有設計參數能整批移植進來、（3）這套框架成為你**自研音色
踏板**的開發平台。

## 1. 願景與問題陳述

lion-heart 的 drive 家族目前 14 顆（`ts9`、`bd2`、`classic`、`centaur`、`evva`、
`red-charlie`、`monster5150`、`angry-charlie`、`jan-ray`、`fuzz-face`、`overdrive`、
`screamer`、`sd1`、`angry-charlie-v2`），其中僅 `screamer` / `sd1` 是 WDF 白箱，
其餘皆 memoryless。使用者回報兩點不滿，兩者其實共根：

- **Tone stack 聽起來像圖形 EQ，不像音箱。** 現行 `drive::ToneStack`
  （`crates/lh-dsp/src/drive/mod.rs`）是三個**獨立、相加、互不干擾**的濾波帶
  （`x + lo·lo + mid·bp + hi·hi`）。真實 Fender/Marshall 被動 tone stack 是一個
  **耦合 RC 網路**：三個旋鈕會互相牽動，且 noon 時**天生有中頻凹陷**（招牌
  scoop）。現行版本 noon 全平、旋鈕正交——這是「不像音箱」的根源。而
  `red-charlie`/`monster5150`/`angry-charlie`/`evva` 的骨架都內建這個 stack，所以
  **修好 tone stack 也連帶修好一半的 drive 不滿**。

- **Drive 缺少真實 clipper 的「反應感」。** memoryless 是瞬時、與頻率無關、與電抗
  元件零互動的靜態曲線；真實削波的靈魂恰恰是它沒有的——RC 與二極體接面互動讓
  削波門檻隨頻率/暫態移動、回授網路隨 drive 變化、對稱性由實際二極體決定。
  `blocks::wdf` 的註解自己講得很清楚，`screamer`/`sd1` 已證明方向對，只是還沒鋪開。

**Tone Revolution 的任務**：把「方向對但零散」的白箱路線，做成「框架化、成規模、
可自研」的音色核心。

## 2. 三大核心目標（驗收此計畫的準繩）

1. **一套完美的 tone stack 框架。** 真實被動 tone stack 的互動與凹陷、涵蓋主要
   機型（Fender Bassman/Twin、Marshall JCM800、Vox AC30、Baxandall、Big Muff
   tone、James/passive），可被任何 drive 複用、也能當獨立 tone/EQ 踏板。→ Phase 02。

2. **把別人設計好的每一顆 drive 參數搬進來（我要所有的 drive）。** 業界名踏板的
   電路拓撲 + 校準過的元件值 + 擬合過的二極體/電晶體參數，整批進 lion-heart。
   → Phase 03–07（依建模技術分家族）。§5 有完整清冊。

3. **框架要能支撐我未來的自研踏板開發。** 從 netlist 到可跑的 Rust 白箱，要有
   工具鏈、擬合流程、驗證 harness 與「新增一顆 WDF 踏板」的食譜。→ Phase 08。

## 3. 架構論點：三層白箱框架

目前 `blocks::wdf` 是**手工化約**的極簡版——只有 `Capacitor`、`DiodePair`/
`AsymDiode`（Newton 解）、`parallel_root`；每個電路都得手推代數化簡成直線程式碼。
這條路對「一兩顆」可行，對「所有的 drive + 完美 tone stack + 自研平台」不夠。要
graduate 成三層：

```
┌─────────────────────────────────────────────────────────────┐
│ 第 3 層  應用：每顆 drive / 每個 tone stack                    │
│   drive::{screamer, sd1, zendrive, rat, mxr, kot, bigmuff…}   │
│   eq::tonestack::{bassman, jcm800, ac30, baxandall…}          │
├─────────────────────────────────────────────────────────────┤
│ 第 2 層  可組合 adaptor（Phase 03）                            │
│   one-ports：Resistor / Capacitor / Res±Cap / V-source        │
│   adaptors：Series / Parallel / **R-Type（散射矩陣 + op-amp）**│
│   線性 tone stack 引擎（Phase 02，解析傳輸函數，可獨立於 WDF） │
├─────────────────────────────────────────────────────────────┤
│ 第 1 層  非線性 root 求解（Phase 01）                          │
│   **Wright Omega 閉式**（取代 Newton）＋ 電晶體/真空管 root    │
└─────────────────────────────────────────────────────────────┘
```

關鍵洞見（來自研究 BYOD 全部 WDF drive）：**op-amp overdrive 家族是同一個核心**
——「WDF 樹 → op-amp R-Type 散射矩陣（op-amp 以有限增益 Ag/輸入阻抗 Ri/輸出阻抗
Ro 建進矩陣）→ 二極體 root」。ZenDrive 的散射矩陣**與 Tube Screamer 一字不差**
（只差零件值與擬合的二極體參數）。所以第 2 層一旦有 R-Type，第 3 層的 TS/SD-1/
ZenDrive/King of Tone/MXR/RAT 幾乎是「換 R/C 值 + 貼散射矩陣 + 設二極體」。

## 4. 移植來源與授權合規（**務必先讀**）

lion-heart 應用碼是 **MIT OR Apache-2.0（寬鬆雙授權）**（見 `Cargo.toml`、
`README.md`）。移植來源授權**不同**，界線必須守住，否則會污染 lion-heart 的授權：

| 來源 | 授權 | 能不能用 | 做法 |
|---|---|---|---|
| `chowdsp_wdf`（WDF 框架、R-Type、adaptor、二極體模型） | **BSD-3** | ✅ 可移植 | 以 Rust 重寫（演算法/結構），保留出處與 BSD 版權宣告 |
| `omega.h`（Wright Omega，D'Angelo） | **MIT** | ✅ 可移植 | 直接以 Rust 重寫，附 MIT 出處 |
| **BYOD 本體**（各 drive/tone 的 `.cpp`/`.h`、Surge waveshaper） | **GPL-3** | ⚠️ **不可整段搬碼** | 見下 |
| 類神經模型權重（Centaur ML、GuitarML、RONN） | 各異/常 GPL | ⚠️ 多半不可散布 | 自行訓練或找寬鬆來源；Phase 07 專章 |

**GPL 界線（BYOD）——什麼是安全的：**

- **電路拓撲、元件值（R/C、二極體型號與 SPICE 參數）＝事實**，不受著作權保護，
  可自由使用。這是「別人設計好的 drive 參數」的合法本體。
- **散射矩陣**：不要複製貼上 BYOD 產生出來的矩陣文字（灰色地帶）。**改用
  R-Solver（`github.com/jatinchowdhury18/R-Solver`）從 netlist 自己重新產生**——
  netlist 是電路圖（事實），R-Solver 的輸出是數學。Phase 08 把這條工具鏈做起來。
- **演算法/技術**（ADAA、WDF 化約、電晶體模型）可自行以 Rust 重新實作；**具體
  GPL 程式碼不可翻譯照抄**。Surge waveshaper 依數學重寫，別搬碼。

> 一句話：**框架與二極體解法從 BSD/MIT 的 `chowdsp_wdf`/`omega.h` 移植；電路的
> "設計參數" 從公開事實（元件值、SPICE model、netlist）取得；GPL 的 BYOD 只當
> 「怎麼做」的教科書，不當「複製來源」。** 每個碰到 GPL 的 Phase 檔都會重申界線。

## 5. 完整 pedal 清冊（目標 2 的範圍）

依**建模技術**分類（也就是 Phase 分家的依據）。lion-heart 已有者標註。

### 5a. op-amp + 二極體 overdrive（同一 WDF 核心，Phase 04）

| 踏板 | 原型 | BYOD 來源 | lion-heart 現況 |
|---|---|---|---|
| Tube Screamer | Ibanez TS808/9 | `drive/tube_screamer`（回授 R-Type + 可選二極體） | 有 `ts9`(memoryless)、`screamer`(WDF shunt) — 可升級為忠實回授拓撲 |
| Boss SD-1 | Boss SD-1 | —（非對稱衍生） | 有 `sd1`(WDF 理想 op-amp) — 可升級為有限增益 R-Type |
| Zen Drive | Hermida Zendrive | `drive/zen_drive`（與 TS 同矩陣、擬合 MOSFET-diode） | 無（`jan-ray`=Timmy 同族，你喜歡這味） |
| King of Tone | Analog Man KoT | `drive/king_of_tone` | 無 |
| MXR Distortion+ | MXR Dist+ | `drive/mxr_distortion`（op-amp R-Type + Ge/Si 二極體） | 無 |
| RAT | ProCo RAT | `drive/mouse_drive`（op-amp R-Type + 濾波網路） | 無 |
| Flapjack | （BYOD 原創） | `drive/flapjack`（op-amp R-Type + 散射矩陣） | 無 |
| Diode Clipper/Rectifier | 通用 | `drive/diode_circuits`（可組態 WDF clipper） | 無（可當「白箱通用 clipper」教學件） |

### 5b. Fuzz / 電晶體 / booster（Phase 05）

| 踏板 | 原型 | BYOD 來源 | 建模技術 |
|---|---|---|---|
| Big Muff | EHX Big Muff Pi | `drive/big_muff`、`drive/muff_clipper` | 級聯電晶體削波級 |
| Fuzz Face | Dallas Arbiter | `drive/fuzz_machine`（`FuzzFaceNDK`） | NDK（節點 DK 法）+ 類神經；lion-heart 已有 behavioral `fuzz-face` |
| Rangemaster | Dallas Rangemaster | `drive/RangeBooster.cpp` | 鍺電晶體 treble booster |
| Bass Face | （Fuzz Face 低音版） | `drive/BassFace.cpp` | 電晶體 fuzz |

### 5c. Memoryless waveshaper（Phase 06）

| 件 | BYOD 來源 | 內容 |
|---|---|---|
| Waveshaper bank | `drive/waveshaper`（Surge） | soft/hard/asym/sine/digital/fold/cheby/fuzz…數十種，**含 ADAA 抗鋸齒** |
| Warp / Blonde | `drive/Warp.cpp`、`drive/BlondeDrive.cpp` | 數位/混合失真 |

### 5d. 類神經 / 真空管（Phase 07，最重、部分暫緩）

| 件 | BYOD 來源 | 依賴 |
|---|---|---|
| Centaur（Klon） | `drive/centaur`（`GainStageML` + WDF 削波 + summing amp） | RTNeural 權重 |
| GuitarML Amp | `drive/GuitarMLAmp.cpp` | RTNeural（LSTM）權重 |
| RONN | `drive/RONN.cpp` | 隨機類神經 |
| Junior B | `drive/junior_b`（`ModifiedRType` + `NeuralTriodeModel`） | 類神經三極管（連白皮書「triode stage」深水題） |
| Tube Amp | `drive/tube_amp` | 真空管級 |

### 5e. Tone stack / EQ（目標 1，Phase 02）

| 件 | BYOD 來源 | 技術 |
|---|---|---|
| Bassman FMV/TMB | `tone/bassman`（WDF 6-port R-Type） | 被動 tone stack（互動 + scoop） |
| Baxandall | `tone/baxandall`（WDF） | Hi-Fi bass/treble |
| TS Tone | `tone/tube_screamer_tone`（WDF） | TS 的 tone 控制 |
| Ladder Filter | `tone/ladder_filter` | Moog 式 LP/HP ladder |

> 誠實界定：5d（類神經/真空管）是最重、且有**權重授權/資產**問題的一塊；本計畫
> 把它排在最後且標為**可選/暫緩**，不阻擋 5a–5c 的高價值主線。「我要所有的 drive」
> 在工程上先由 5a/5b/5c 兌現絕大多數，5d 視資源與授權再議。

## 6. Phase 藍圖總表

| # | Phase | 命中目標 | 依賴 | 產出 | 規模 |
|:-:|---|:-:|:-:|---|:-:|
| 01 | 快速非線性 root（Wright Omega） | 2 的成本 | — | `blocks::wdf` 加 omega 解、A/B、bench | 小 |
| 02 | **Tone stack 框架** | **1** | — | `eq::tonestack` 解析引擎 + 機型註冊表；換掉 `ToneStack` | 中 |
| 03 | WDF 可組合 adaptor + R-Type + op-amp | 2/3 地基 | 01 | 第 2 層框架；新 ADR | 大 |
| 04 | op-amp overdrive 家族 | 2 | 03 | TS/SD-1/ZenDrive/KoT/MXR/RAT/DiodeClipper + 可選二極體 | 大 |
| 05 | Fuzz/電晶體/booster 家族 | 2 | 03 | BigMuff/FuzzFace/Rangemaster/BassFace | 中 |
| 06 | Waveshaper bank + ADAA | 2 + 品質 | — | waveshaper 踏板 + 既有 drive 抗鋸齒改造 | 中 |
| 07 | 類神經/真空管家族（可選） | 2 | 神經路徑 | Centaur/GuitarML/triode… | 大 |
| 08 | **自研平台工具鏈** | **3** | 03 | netlist→R-Solver→codegen、SPICE 擬合、驗證 harness、食譜 | 中 |

**建議執行順序與理由：**

1. **01 → 02**：先做 Wright Omega（讓 WDF 從「奢侈品」變「日常」），與 tone stack
   框架（命中最明確的不滿、連帶改善多顆 drive）。兩者都不大、彼此獨立、立刻有感。
2. **03**：架好可組合 adaptor + R-Type + op-amp——這是「所有 op-amp drive」與「自研
   平台」的共同地基。**架構級改動，需新 ADR。**
3. **04 → 05 → 06**：依家族鋪開 drive；06（waveshaper/ADAA）可與任一步平行，因為它
   不依賴 WDF 框架。
4. **08**：框架穩定後把工具鏈做起來，交付「自研」能力。
5. **07**：最後、可選；連結白皮書 triode 深水題與 ADR 027 跨平台。

每個 Phase 的**具體工作內容**見 `phase/NN-*.md`。

## 7. 跨階段共同決策（每個 Phase 都適用）

- **RT 規則不可破**（CLAUDE.md §即時音訊規則）：audio thread 上零配置、零鎖、
  無 syscall；WDF 樹在 `prepare` 建好；迭代/矩陣維度上界固定；denormal flush；
  非有限輸出在 debug build assert。新踏板一律過 `assert_no_alloc`。
- **Append-only**：新踏板追加進 `MODELS`/`DRIVE_PEDALS`（`ModelDef` = desc +
  `Ctl` routing + build fn），**盡量不 bump preset schema**；plugin 由
  `from_families` 自動展開參數（**pre-v0.1 additive id 變動 → 重跑 clap-validator**）。
- **升級既有 vs 新增**：`screamer`/`sd1`/`fuzz-face` 的「忠實版」以**新 key 追加**
  （保 preset/plugin id 穩定），或在 ADR 明確記錄為 append-only。Tone stack 例外——
  它是**共用建構塊**，換掉會改變既有 FMV 系 drive 的聲音，這是**使用者想要的
  voicing 改善**；每顆被重調的 drive 其 character 測試須更新並重新 pin（見 Phase 02）。
- **測試**：每個 WDF 核心要有（a）解方程殘差 `a = v + R·i(v)`、（b）對稱/非對稱、
  （c）飽和/有界（±1e6 狂推不 NaN）、（d）靜態轉移曲線對照離線高精度參考、
  （e）**白箱判別測試**（頻率相依削波——memoryless 不成立的行為）、（f）多 rate/
  block、silence→silence。
- **Bench**：每顆進 `cargo bench -p lh-dsp`，成本記入 `docs/benchmarks.md` 深水區段。
- **ADR/PRD**：架構級（Phase 02 tone stack 引擎、Phase 03 WDF 框架、Phase 07 神經
  路徑）各開一支 ADR；每個 Phase 對應一份 PRD（本目錄）。既有編號續接
  （PRD 022+、ADR 030+）。
- **4× Oversample**：削波前沿用家族 `Oversampler4x`；若二極體轉角在 4× 下抗混疊
  不足，評估 8×（記入 ADR）。

## 8. 成功指標

- **目標 1**：換上真實 tone stack 後，同一顆 FMV 系 drive 在 noon 有可量測的中頻
  凹陷、三旋鈕互動可量測（轉 bass 改變 treble 響應）；耳朵上「像音箱不像 EQ」。
- **目標 2**：op-amp overdrive 家族（≥6 顆：TS/SD-1/ZenDrive/KoT/MXR/RAT）＋ fuzz/
  電晶體家族（≥3 顆）以白箱進 registry，各有 character pin 與 bench，全綠。
- **目標 3**：一份可跑的「netlist → 散射矩陣 → Rust 白箱」流程 + 驗證 harness；
  使用者能照食譜加一顆自己的 WDF 踏板（Phase 08 附範例）。
- **全程**：`cargo fmt/clippy/test` 全綠；`assert_no_alloc` 靜默；RTL/CPU 在預算內。

## 9. 非目標

- **不追 SPICE 位元級對拍**——目標是靜態曲線在容差內、動態行為可量測地優於
  memoryless、耳朵更像真踏板。
- **不做整顆踏板的每一級**（電源、旁通、緩衝）——只做決定音色的關鍵級（削波、
  tone stack、關鍵濾波），沿用 lion-heart 現有的 `shape()/post()/eq()` 分工。
- **不在 v1 動 engine/session 訊息集**——除非 Phase 07 神經路徑逼不得已（另議）。
- **不散布任何 GPL 或授權不明的模型權重/程式碼**（見 §4）。
- **本計畫不含 cab/IR、reverb、mod 等非 drive/tone 家族**——那些已完成或另有路線。

## 10. 詞彙表

- **WDF（Wave Digital Filter）**：把類比電路離散進「波域」（`a = v+Ri`、`b = v−Ri`）
  的方法；線性元件成 one-port，單一非線性放樹根，線性部分對它呈 Thévenin 等效。
- **R-Type adaptor**：處理無法用 series/parallel 化約的拓撲（如含 op-amp 回授的
  網路）的 N-port 適配器；核心是一個 N×N **散射矩陣** `b = S·a`，S 由各 port 阻抗
  算出（公式由 R-Solver 產生）。
- **Wright Omega**：解 `ω + ln(ω) = x` 的函數（Lambert W 的 e^x 版）；WDF 二極體
  方程 `a = v + R·i(v)` 可重排成一次 ω 求值 → 取代 Newton 迭代，零迭代、branch-free。
- **FMV / TMB tone stack**：Fender/Marshall/Vox 共用的被動三旋鈕（Treble-Middle-
  Bass）tone 網路；招牌中頻凹陷 + 強旋鈕互動。
- **ADAA（Antiderivative Anti-Aliasing）**：用波形整形函數的反導數做抗鋸齒，比純
  oversample 更有效抑制硬切產生的鋸齒。
- **NDK（Nodal DK method）**：另一種電路離散法（狀態空間），BYOD 的 Fuzz Face 用它。
- **R-Solver**：ChowDSP 的 Python 工具，從電路 netlist 自動產生 R-Type 散射矩陣。
</content>
</invoke>
