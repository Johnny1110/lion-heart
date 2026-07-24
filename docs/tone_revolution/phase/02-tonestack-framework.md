# Phase 02 — Tone Stack 框架：真實被動音色網路

命中目標：**#1（完美的 tone stack 框架）** · 連帶改善多顆 FMV 系 drive
依賴：無（解析路徑不需 WDF 框架，可與 Phase 01 平行）
關聯 ADR：**新開 ADR 030（tone stack 引擎架構）**
來源與授權：技術＝David Yeh & Julius Smith,「Discretization of the '59 Fender
Bassman Tone Stack」(DAFx-06) 與 Yeh 博論（公開學術）；元件值由 BYOD
`tone/bassman/BassmanToneStack.h` 佐證（元件值＝事實）。**不搬 GPL 碼。**

---

## 1. 背景與決策

現行 `drive::ToneStack`（`crates/lh-dsp/src/drive/mod.rs`）是三個**獨立相加**的
濾波帶：

```rust
*s = x + lo_gain*lo + mid_gain*bp + hi_gain*hi;   // 圖形 EQ，不是 tone stack
```

真實 Fender/Marshall/Vox 被動 tone stack 是一個**耦合 RC 網路**，有兩個現行版本
完全沒有、卻是「像音箱」的關鍵：

1. **旋鈕互動**：Bass/Mid/Treble 共享同一組電容與節點，轉一個會牽動另外兩個的
   響應。現行版本三旋鈕正交。
2. **noon 天生中頻凹陷**（招牌 scoop）＋ 插入損耗：真實 stack 在所有旋鈕置中時
   **不是平的**；現行版本 noon 全平 → 「乾淨 hi-fi EQ 感」。

**關鍵工程判斷——被動 tone stack 是線性網路（純 R/C/電位器，無非線性元件）**，
所以有兩條等價但成本不同的路：

- **路線 A（解析傳輸函數，本 Phase 主線）**：對電路做節點分析得
  `H(s) = N(s; l,m,t) / D(s; l,m,t)`——係數是三個旋鈕位置 `l,m,t` 與元件值的函數
  ——再 bilinear 離散成一個**三階數位濾波器**，旋鈕動時在 block-rate 重算係數。
  對線性網路這與 WDF **數學等價**，但**便宜得多**（一個三階濾波器，無逐 sample
  矩陣乘、無非線性解）。這是經典「Tone Stack Calculator」/ Yeh 的做法。
- **路線 B（WDF R-Type）**：把網路建成 WDF 6-port R-Type（BYOD Bassman 的做法）。
  只有在 tone 網路**含非線性元件**（例如白皮書 roadmap 的「diode-ladder tone
  stack」）時才需要——那時交給 Phase 03 的 R-Type，本 Phase 先不做。

**拍板**：本 Phase 交付**路線 A 的解析 tone stack 引擎** `eq::tonestack`，涵蓋主要
機型，成為（a）drive 內部 `post()` 的共用建構塊、（b）可選的獨立 tone 踏板。非線性
diode-ladder tone stack 留待 Phase 03 R-Type 落地後再議。

## 2. 規格

### 2.1 `eq::tonestack` 引擎（新，`crates/lh-dsp/src/eq/tonestack.rs`）

- **拓撲通式**：FMV/TMB 網路的三階 `H(s)`。以 Yeh 的推導，分子/分母各為 `s` 的
  三次多項式，係數 `b1..b3, a1..a3` 是 `(l, m, t, R1..R4, C1..C3)` 的封閉式。
  提供一個 `struct ToneStackModel { r1..r4, c1..c3 }` 持有元件值，一個
  `coeffs(l, m, t) -> ([b0..b3],[a0..a3])` 算類比係數。
- **離散化**：對 `H(s)` 做 bilinear（`s → 2fs·(1−z⁻¹)/(1+z⁻¹)`），得三階數位
  IIR（或兩級聯：一 biquad + 一 one-pole，數值較穩）。旋鈕移動時於 block-rate
  重算並交叉淡入（沿用 lion-heart `eq::global` 的 settled-skip：旋鈕不動就跳過
  係數重建）。
- **RT 安全**：係數重算在控制/block 邊界，不在 audio 熱迴圈逐 sample；狀態
  denormal flush；係數變動經平滑，避免 zipper。

### 2.2 機型註冊表 `ToneStackKind`

一個 append-only 註冊表，每個機型是一組元件值（＝事實，可查 schematic）：

| Kind | 原型 | 特徵 | 元件值來源 |
|---|---|---|---|
| `Bassman` | Fender 5F6-A / Twin | 標準 FMV，中頻凹陷深 | BYOD Bassman：R1 250k, R2 1M, R3 25k, R4 56k, C1 250pF, C2/C3 20nF |
| `JCM800` | Marshall 2203/2204 | 亮、mid 較前 | Marshall schematic（公開） |
| `AC30` | Vox AC30「cut/tone」 | 不同拓撲（Vox 較簡） | Vox schematic |
| `Baxandall` | Hi-Fi bass/treble | 對稱、無 scoop、平坦可調 | 標準 Baxandall |
| `BigMuffTone` | EHX Big Muff | 中頻凹陷「wah 反相」 | Big Muff schematic |
| `James`/`Passive` | 通用被動 | 簡單兩旋鈕 | 標準 |

> `Bassman`/`JCM800`/`AC30` 同屬 FMV 通式、只差元件值與（AC30）少數拓撲支路——
> 佐證了「一個引擎、換零件＝多機型」。

### 2.3 與既有 drive 的整合（voicing 改動，使用者要的）

現行用 `ToneStack` 的 drive：`evva`、`red-charlie`、`monster5150`、`angry-charlie`、
`angry-charlie-v2`（Baxandall/Marshall 系）。兩個選項：

- **(i) 直接遷移**（建議）：把這些 drive 的 3-band `eq()`/`post()` 換成真實
  `ToneStackKind`（`red-charlie`/`monster5150`→`JCM800`；`angry-charlie`系→
  Baxandall/JCM800；`evva`→其設計對應機型）。**這會改變它們的聲音**——正是
  使用者想要的改善。每顆的 character 測試（EQ-band、tilt、scoop）須**更新並重新
  pin**；ADR 記錄「voicing 改動，pre-v0.1，非 append-only 相容」。
- **(ii) 新增變體**：保留舊 drive，另以新 key 追加「real-stack 版」。保 preset
  穩定但 registry 膨脹。

> 建議 (i)——使用者明確要更好的音色，且 pre-v0.1 正是改 voicing 的時機。以測試
> 重新 pin 新特徵、ADR 交代清楚即可。若使用者想保舊聲音再走 (ii)。

### 2.4 獨立 tone 踏板（可選，複用引擎）

lion-heart `eq` 家族已有 `chain`(3-band) 與 `parametric`。可再追加一顆
`tonestack` pedal（faceplate：Bass/Mid/Treble + 機型選擇 stepped param），把真實
音箱 tone stack 當獨立效果器用（放在 amp 前/後皆可）。append-only 進 `eq` 家族，
無 schema bump。

## 3. 非目標

- **不做非線性 diode-ladder tone stack**（那需 Phase 03 R-Type）——本 Phase 純線性。
- **不追每台真機的元件公差**——用代表性 schematic 值，聽感校準後 pin。
- **不改 engine/session/plugin 訊息集**——tone stack 是 DSP 建構塊 + 一顆可選踏板。
- 不移植 BYOD 的 WDF Bassman **程式碼**（GPL）——用公開的傳輸函數推導自行實作。

## 4. 驗收標準

### 4.1 `cargo test`
- **旋鈕互動**（白箱判別）：固定 Bass/Treble，掃 Mid，量測 Treble 頻段響應**有
  變化**（證明耦合）；對照現行 `ToneStack` 此測試不成立。
- **中頻凹陷**：`Bassman`/`JCM800` 在 noon（l=m=t=0.5）於 ~400–800 Hz 有可量測
  凹陷（相對 100 Hz/3 kHz）；`Baxandall` noon 近平坦（對照組）。
- **傳輸函數對照**：數位 `H(z)` 的頻率響應對照解析 `H(s)`（bilinear 預畸校正後）
  在音訊帶內容差內。
- **極端旋鈕穩定**：三旋鈕全掃（0/0/0 到 1/1/1）係數有界、濾波器穩定（極點在
  單位圓內）、無 NaN、無 zipper（平滑後）。
- **多 rate/block**（44.1/48/96 kHz、block 32–1024）、bypass/flat 行為明確、
  silence→silence。
- **遷移對照**（若採 2.3(i)）：被重調 drive 的新 character pin 全綠。

### 4.2 `cargo bench`
- `tonestack_fmv` 每 64-frame block 成本（預期 ~biquad 級，遠低於 WDF）；settled-
  skip 生效時近乎免費。記入 `docs/benchmarks.md`。

### 4.3 `assert_no_alloc`
- 掃旋鈕（觸發係數重算）全程無配置。

### 4.4 耳朵（使用者）
- 同一顆 FMV 系 drive，換真實 stack 前後 A/B：noon 是否「有音箱的凹陷骨架」；
  三旋鈕是否互動（轉 bass 感覺 treble 也變）；掃 Mid 是否聽到 scoop 移動；
  整體「像音箱不像圖形 EQ」。
- 獨立 tonestack 踏板放 amp 前/後，各機型（Bassman/JCM800/AC30/Baxandall）辨識度。

## 5. 產出清單

- `crates/lh-dsp/src/eq/tonestack.rs`：引擎 + `ToneStackKind` 註冊表 + bilinear 離散。
- （2.3）遷移 `evva`/`red-charlie`/`monster5150`/`angry-charlie*` 的 tone 級 +
  更新 character 測試。
- （2.4 可選）`eq` 家族追加 `tonestack` 踏板 + livery + plugin id 展開。
- **ADR 030**：tone stack 引擎（路線 A 解析傳輸函數 vs 路線 B WDF；遷移決策；
  非線性留 Phase 03）。
- **PRD 022**（本檔的正式 PRD 版，若要進 `docs/PRD/` 主序列）。
- 更新 `docs/benchmarks.md`。

## 6. 風險與備註

- **三階 IIR 在低取樣率的數值穩定**：bilinear 在 Nyquist 附近有頻率壓縮；tone
  stack 轉角多在中低頻，影響小；必要時對關鍵轉角做 bilinear 預畸（prewarp）。
- **係數封閉式冗長**：Yeh 的 Bassman 係數式很長但一次寫對即可；建議附一支離線
  測試，用符號/數值 SPICE 對照一組旋鈕點驗證係數正確。
- **這是使用者最有感的一步**，且不依賴大框架——建議緊接 Phase 01 之後、或平行做。
</content>
