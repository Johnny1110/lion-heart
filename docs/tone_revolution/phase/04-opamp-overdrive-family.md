# Phase 04 — op-amp overdrive 家族（WDF）

命中目標：#2（別人設計好的 drive 參數——overdrive 主力）
依賴：Phase 03（R-Type + op-amp）、Phase 01（omega root）
關聯 ADR：沿用 ADR 031（框架）；各踏板 append-only，無新架構 ADR（除非個別踏板
需要，如可選二極體 UX）
來源與授權：電路拓撲＋元件值＋二極體 SPICE 參數＝**事實**（可查 schematic /
BYOD 佐證）；散射矩陣由 **R-Solver 自產**。**不搬 BYOD(GPL) 碼。**

---

## 1. 背景與決策

Phase 03 落地後，op-amp overdrive 家族共用同一核心：

```
WDF 樹（RC 網路）→ op-amp R-Type 散射矩陣（Ag/Ri/Ro）→ 二極體 root（omega）
```

BYOD 已證明 TS/ZenDrive/MXR/RAT 是「同核心換零件」。本 Phase 把這一整族以白箱
搬進 lion-heart registry，每顆 append-only（`MODELS`/`DRIVE_PEDALS` 追加、
`ModelDef` = desc + `Ctl` routing + build fn、plugin 自動展開）。

**拍板**：交付 ≥6 顆 op-amp 白箱 overdrive + **可選二極體 UX**。每顆＝(a) 從公開
schematic 建 netlist、(b) R-Solver 產散射矩陣、(c) 在 `blocks::wdf` 拼樹、(d) 設
二極體/元件參數、(e) `shape()` 每 4× OS sample 解一次 WDF、`post()` 走 tone tilt +
makeup + DC block。

## 2. 規格：踏板清單與設計參數

> 元件值主要佐證自 BYOD 對應檔（＝schematic 事實）。散射矩陣**不抄**，用 R-Solver
> 從同一 netlist 重產。二極體 `Is/n/Vt` 為 SPICE 代表值，聽感校準後 pin。

### 2.1 Tube Screamer（忠實回授拓撲）— 升級 `screamer`/`sd1` 或新增 `ts-wdf`
BYOD `drive/tube_screamer`：op-amp（Ag=100, Ri=1e9, Ro=0.1）回授迴路內削波。
- 回授：`R6=51k` 串 `Pot1=500k`（drive），並聯 `C4=51pF`（drive 轉大變暗）。
- 輸入級：`C2=1µF`、`R5=10k`；`R4=4.7k` 串 `C3=0.047µF`（RC 塑形）。
- 二極體：1N4148，`Is≈4.35e-9, nVt≈1.906×Vt`（BYOD 擬合的非整數二極體數）。
- 負載 `RL=1M`。輸入 −6 dB、輸出 −6 dB（BYOD 的 headroom trim）。
> 相對 lion-heart 現有 `screamer`（shunt）/`sd1`（理想虛短）：這是**有限增益 op-amp
> 回授**的忠實版，三者可 A/B（memoryless `ts9` / shunt / 理想虛短 / 有限增益回授）。

### 2.2 Zen Drive（Hermida Zendrive）— 新增 `zendrive`
BYOD `drive/zen_drive`：**與 TS 同一散射矩陣**（op-amp R-Type），差在零件與二極體。
- 輸入 `C3=470nF`、`R4=470k`；voice 級 `R5=1k + R6=10k`（voice 鈕）串 `C5=100nF`。
- gain 級 `R9=500k×gain` 並聯 `C4=100pF`；`RL=1M`。
- 二極體：**擬合的 MOSFET-as-diode**，`Is≈5.241e-10, nVt≈0.0787`（BYOD 由 SPICE
  暫態擬合——見 `sim/ZenDrive/`）。這是「Timmy 同族、透明動態」音色，roll back
  吉他音量清乾淨；你已有 `jan-ray`(Timmy) 會喜歡這味。
- Faceplate：Gain / Voice / (Tone) / Level。

### 2.3 King of Tone（Analog Man）— 新增 `king-of-tone`
BYOD `drive/king_of_tone`（`KingOfToneClipper` + `KingOfToneOverdrive`）：op-amp
overdrive + 二極體 clipper，透明中增益。元件值查 KoT/Marshall Bluesbreaker 系
schematic（KoT 源自 Bluesbreaker 拓撲）。可含「boost/overdrive」兩模式（stepped）。

### 2.4 MXR Distortion+ — 新增 `mxr-dist`
BYOD `drive/mxr_distortion`：op-amp R-Type（Ag=100, Ri=1e9, Ro=0.1）+ 二極體對。
- `R4=1M`；輸入 `C1=1nF`、`R1=10k + C2=10nF`；`Vb=4.5V` bias（1M）。
- dist 鈕：`R3=4.7k + rDist=1M × dist` 串 `C3=47nF`；輸出 `R5=10k + C4=1µF`、
  `Rout=10k`、`C5=1nF`。
- 二極體：`Is=2.52e-9, nVt=1.75×Vt`（germanium/GZ34 味）。硬、中頻凸、經典
  distortion+。

### 2.5 RAT（ProCo）— 新增 `rat`
BYOD `drive/mouse_drive`：op-amp R-Type（Ri=10M）+ 二極體對（`Is=5e-9, nVt=2×Vt`）。
- 輸入 `C1=22nF`、`R2=1M`（bias 4.5V）；`R3=1k` 串 `C2=1nF`。
- filter 級（RAT 招牌）：`R4=47Ω+C5=2.2µF`、`R5=560Ω+C6=4.7µF` 並聯；dist
  `Rdist=100k`（×0.5）並聯 `C4=100pF`；輸出 `R6=1k+C7=4.7µF`。
- RAT 的「Filter」鈕（其實是 tone 的 LP）要保留——暗、粗、liquid lead。

### 2.6 Diode Clipper / Rectifier（通用白箱）— 新增 `diode-clipper`
BYOD `drive/diode_circuits`：可組態 WDF 二極體 clipper（對稱/整流、可選二極體）。
當作「教學件 + 自研起點」——最能展示 Phase 08 平台。

### 2.7 可選二極體 UX（給 TS 系）
BYOD `DiodeParameter.h`：一顆 TS 可切二極體型 + 二極體數，低成本高感知價值。
- 型別（`Is`）：`GZ34 2.52n` / `1N34(germanium) 200p` / `1N4148(silicon) 2.64n`。
- 數量：連續 `#diodes 0.3–3.0`（非整數＝emissivity 微調）。
- 實作：`DiodePair`/`AsymDiode` 已吃 `is/n/vt`；加一個 stepped「diode」param + 一個
  「count」knob，routing 進 `Ctl`（可能需擴 `Ctl` enum 或走 per-pedal ctl 表，
  照 delay/reverb 的 `Ctl` 表前例）。**若需新參數語彙 → 評估是否 schema bump**
  （傾向 append-only param、無 bump）。

## 3. 非目標

- **不做整顆踏板每一級**（電源/旁通/緩衝）——只做削波 + 關鍵 tone/filter 級。
- **不做類神經 Centaur/GuitarML**（Phase 07）——本 Phase 是純電路 op-amp 家族。
- **不追 SPICE 位元對拍**——靜態曲線容差內 + 動態白箱行為 + 耳朵。
- **不抄散射矩陣文字**——一律 R-Solver 自產（Phase 08 工具）。

## 4. 驗收標準（每顆踏板）

### 4.1 `cargo test`
- **解方程殘差**：WDF root `a = v + R·i(v)` 容差內、有界、無 NaN（±1e6 狂推）。
- **靜態轉移曲線**：DC 掃描對照離線高精度電路解在容差內。
- **白箱判別**：頻率相依削波（高頻等效門檻 ≠ 低頻）——memoryless 不成立。
- **character pin**（每顆一條，凸顯其身分）：
  - `zendrive`：低增益透明、roll-back 清乾淨（動態比 `rat`/`mxr` 高）。
  - `rat`：暗（Filter LP）、比 TS 粗、二極體對稱奇次為主。
  - `mxr-dist`：中頻凸、硬。
  - `ts-wdf`：mid-hump、drive 轉大變暗（51pF）。
  - 可選二極體：切 germanium vs silicon 有可量測轉角差。
- 多 rate/block、silence→silence、bypass 透明。

### 4.2 `cargo bench`
- 每顆進 `lh-dsp` bench（omega + 4× OS + R-Type 矩陣乘）；記 `docs/benchmarks.md`。
  預期同 `screamer`(omega 後) 量級。

### 4.3 `assert_no_alloc`
- 每顆 select + 狂推 + 掃旋鈕（含切二極體型）零配置。

### 4.4 耳朵（使用者）
- 逐顆對真踏板/名 demo A/B：TS 的 mid-hump、ZenDrive 透明動態、RAT 的暗粗 lead、
  MXR 的硬中頻、KoT 透明；可選二極體切 Ge/Si 的手感差。

## 5. 產出清單

- `crates/lh-dsp/src/drive/{zendrive,king_of_tone,mxr_dist,rat,diode_clipper}.rs`
  + 忠實版 TS（升級或新 key）。
- 每顆的 netlist（進 `sim/` 或 `tools/netlists/`，供 R-Solver 重產矩陣）。
- registry 追加（`MODELS`/`DRIVE_PEDALS`/`MODEL_COUNT`）、theme livery（distinct-
  livery pin）、plugin id 展開（**pre-v0.1 additive，重跑 clap-validator**）。
- 可選二極體 param + UI。
- character/bench 測試；更新 `docs/benchmarks.md`。
- **PRD 024**（正式版，若進主序列）。

## 6. 風險與備註

- **元件值來源**：優先用公開 schematic；BYOD 值當佐證/交叉驗證，不當唯一來源。
- **可選二極體的 param routing**：`Ctl` enum 目前是固定六種；擴充要照既有 per-pedal
  ctl 表前例，別破壞 append-only。
- **一次一顆 PR**：先做 ZenDrive（你最可能喜歡、且與 TS 同矩陣＝最省），驗證流程
  順了再鋪其餘。
</content>
