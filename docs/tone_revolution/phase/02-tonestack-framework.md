# Phase 02 — Tone Stack 框架：真實被動音色網路

> **已實作（2026-07-27）— PRD 023 / ADR 030。** 以下是本檔案（計畫）與實際交付的
> 三處差異，計畫本身不改，差異記在這裡：
>
> 1. **§2.1 的實作形式改了。** 計畫寫「每種拓撲手推一組封閉式係數 → bilinear 成
>    三階直接式 IIR」；實際交付是 **netlist（資料）→ 數值解出連續狀態空間 →
>    Tustin**。仍是路線 A（線性網路，非 WDF），但穩定性變成結構性保證（被動 RC
>    極點必為實負 ⇒ Tustin 必落單位圓內），§2.1 準備的「必要時升級成級聯 + 解三次
>    實根」後備因此不需要；且加機型＝加一份 netlist，不動引擎。代價實測 +20 %
>    （旋鈕狂轉時），完整論證見 ADR 030。
> 2. **§2.2 的六個機型只交付三個**：`bassman`、`jcm800`、`big-muff`。
>    **`AC30`/`Baxandall`/`James` 暫緩**——無法可靠佐證其拓撲與元件值，不編造。
>    （實測把 AC30 的 mid leg 猜成固定 10k 會得到比 Bassman 更深的凹陷，與 Vox
>    「mid 前傾」的公認特徵矛盾 ⇒ 是猜錯了值。）註冊表是 append-only 純資料。
>    §4.1 原以 `Baxandall`「noon 近平坦」當對照組，改由跨機型判別取代
>    （Bassman 凹陷須比 JCM800 深 ≥1 dB；Big Muff 的凹陷位置隨旋鈕滑動而 FMV 不會）。
> 3. **§4.1 的 ngspice fixtures 換成等價物**：ngspice 不是本專案的建置相依，改以
>    一支**獨立的複數節點分析 oracle**（寫在測試模組、與受測路徑不共用程式碼）當
>    golden。涵蓋任意旋鈕點且跟著 CI 跑。
>
> 另外 §2.1 的 pot taper 機制已實作並測試，但三個機型目前都設 `Linear`——理由見
> PRD 023 §5。

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

- **每種拓撲一組封閉式**（v2 修正：不是單一通式吃所有機型）：
  - **FMV/TMB 通式**（三階）：以 Yeh 的推導，分子/分母各為 `s` 的三次多項式，
    係數是 `(l, m, t, R1..R4, C1..C3)` 的封閉式。涵蓋 Bassman/JCM800/AC30
    Top Boost（AC30 為同拓撲、mid 電阻固定值）。
  - **Baxandall**（bass/treble 兩旋鈕，二階×2 或三階，自己的封閉式）。
  - **Big Muff tone**（單旋鈕 LP/HP blend，一階×2 的簡式）。
  - **James/passive**（兩旋鈕，自己的式子）。
  `ToneStackModel` = 拓撲 tag + 該拓撲的元件值；`coeffs(knobs) -> 類比係數`
  按拓撲分派。
- **pot taper（v2 新增，聽感關鍵）**：真機的 bass/treble 多為 audio taper、
  mid 為 linear——旋鈕位置 0..10 → pot 分數的映射要按 kind 帶 taper 法則，
  否則「noon」對不上真機的 noon（招牌凹陷會偏位）。taper 進 `coeffs` 之前的
  knob-map 層，per-kind 標定。
- **離散化**：對 `H(s)` 做 bilinear（`s → 2fs·(1−z⁻¹)/(1+z⁻¹)`），得**三階直接式
  IIR（transposed DF2）**。係數推導與 bilinear 代換全程 **f64**、落地前下轉
  f32（三次多項式係數對元件值的組合在 f32 會損位）。
  - v2 修正：初稿的「兩級聯（biquad + one-pole）數值較穩」**代價沒講**——級聯
    需要在每次旋鈕動時對分子/分母**解三次實根**再配對，block-rate 做 Cardano/
    Newton 不是不行，但先不背這個複雜度；tone stack 極點都在中低頻、Q 低，
    f64 推導 + TDF2 直接式在 44.1–96k 預期穩定，**以 §4.1 的極點半徑測試守住**；
    真的出數值問題再升級成級聯（記 ADR）。
- 旋鈕移動時於 block-rate 重算 + 係數平滑（沿用 `eq::chain`（global EQ 同箱）的
  settled-skip：旋鈕不動就跳過重建）。
- **RT 安全**：係數重算在控制/block 邊界，不在 audio 熱迴圈逐 sample；狀態
  denormal flush；係數變動經平滑，避免 zipper。
- **插入損與 makeup（v2 新增）**：真實被動 stack 在 noon 有實打實的插入損
  （依機型 −6 到 −20 dB）——per-kind 帶一個固定 makeup 增益，讓遷移後的 drive
  過既有 level-norm pin（`modelled_pedals_sit_near_unity_at_default_knobs`），
  獨立踏板版預設也近 unity。

### 2.2 機型註冊表 `ToneStackKind`

一個 append-only 註冊表，每個機型是一組元件值（＝事實，可查 schematic）：

| Kind | 原型 | 特徵 | 元件值來源 |
|---|---|---|---|
| `Bassman` | Fender 5F6-A / Twin | 標準 FMV，中頻凹陷深 | **原機 5F6-A schematic**：treble 250k(audio), bass 1M(audio), mid 25k(linear), slope 56k, C1 250pF, C2/C3 20nF。⚠️ v2 查核：BYOD 的 `R3=96k` 是**它自己改的**（原始碼註解 `modified from 25e3`）——我們以原機為準，BYOD 值只當「他們的 voicing 選擇」參考 |
| `JCM800` | Marshall 2203/2204 | 亮、mid 較前 | Marshall schematic（公開）：slope 33k, C1 470pF, C2/C3 22nF, treble 220k, bass 1M, mid 22k（型號有差，取代表值） |
| `AC30` | Vox AC30 Top Boost | 同 FMV 拓撲、**mid 固定**（無 mid 旋鈕），值不同 | Vox Top Boost schematic |
| `Baxandall` | Hi-Fi bass/treble | 對稱、無 scoop、平坦可調 | 標準 Baxandall |
| `BigMuffTone` | EHX Big Muff | 中頻凹陷「wah 反相」 | Big Muff schematic（39k/22nF ‖ 100k/10nF 系，版本眾多取代表值） |
| `James`/`Passive` | 通用被動 | 簡單兩旋鈕 | 標準 |

> `Bassman`/`JCM800`/`AC30` 同屬 FMV 通式、只差元件值（AC30 的 mid 為定值）——
> 佐證了「一個引擎、換零件＝多機型」。所有元件值都是可查 schematic 的事實；
> **BYOD 只做交叉驗證，不做唯一來源**（Bassman R3 一案正是教訓）。

### 2.3 與既有 drive 的整合（voicing 改動，使用者要的）

現行用 `ToneStack` 的 drive：`evva`、`red-charlie`、`monster5150`、`angry-charlie`、
`angry-charlie-v2`（Baxandall/Marshall 系）。兩個選項：

- **(i) 直接遷移**（建議）：把這些 drive 的 3-band `eq()`/`post()` 換成真實
  `ToneStackKind`（`red-charlie`/`monster5150`→`JCM800`；`angry-charlie`系→
  Baxandall/JCM800；`evva`→其設計對應機型）。**這會改變它們的聲音**——正是
  使用者想要的改善。每顆的 character 測試（EQ-band、tilt、scoop）須**更新並重新
  pin**；ADR 記錄「voicing 改動，非 append-only 相容」。
- **(ii) 新增變體**：保留舊 drive，另以新 key 追加「real-stack 版」。保 preset
  穩定但 registry 膨脹。

> 建議 (i)——使用者明確要更好的音色；v0.1.0 雖已釋出，但目前單人使用、preset
> 都在本機，voicing 改動以「ADR 交代 + 測試重新 pin」處理即可，不需相容性包袱。
> 若某顆的舊聲音真想留再走 (ii)。
>
> 實作備註：既有 `Circuit::eq()` 收的是逐 sample 旋鈕軌跡（`low/mid/high:
> &[f32]`）；解析 stack 的係數重算在 block 邊界取軌跡端點即可（係數再平滑），
> 不必逐 sample 重算——與 `eq::chain` 的既有慣例一致。

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
- **係數正確性 golden（v2 提升為驗收，這是最容易寫錯的地方）**：ngspice 對每個
  kind 跑 AC 掃描（幾組代表旋鈕點：noon、全開、全關、混合），輸出存成 repo
  fixtures；我們的 `H(s)` 幅度響應對照 fixtures 在容差內（bilinear 預畸後音訊帶
  內）。封閉式係數「一次寫對」靠這個，不靠肉眼。
- **旋鈕互動**（白箱判別）：固定 Bass/Treble，掃 Mid，量測 Treble 頻段響應**有
  變化**（證明耦合）；對照現行 `ToneStack` 此測試不成立。
- **中頻凹陷**：`Bassman`/`JCM800` 在 noon（taper 映射後的真 noon）於
  ~400–800 Hz 有可量測凹陷（相對 100 Hz/3 kHz）；`Baxandall` noon 近平坦
  （對照組）。
- **傳輸函數對照**：數位 `H(z)` 的頻率響應對照解析 `H(s)`（bilinear 預畸校正後）
  在音訊帶內容差內。
- **極端旋鈕穩定**：三旋鈕全掃（0/0/0 到 1/1/1）係數有界、濾波器穩定（**極點
  半徑 < 1，44.1/48/96k 各驗**）、無 NaN、無 zipper（平滑後）。
- **多 rate/block**（44.1/48/96 kHz、block 32–1024）、bypass/flat 行為明確、
  silence→silence。
- **遷移對照**（若採 2.3(i)）：被重調 drive 的新 character pin 全綠，**且既有
  level-norm pin 全綠**（插入損由 per-kind makeup 補償，見 §2.1）。

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
- **ADR**（暫定 030）：tone stack 引擎（路線 A 解析傳輸函數 vs 路線 B WDF；
  遷移決策；非線性留 Phase 03）。
- **PRD**：落地時於主序列取號。
- ngspice AC 掃描 fixtures（`sim/tonestack/` 或測試 fixtures 目錄）＋產生腳本。
- 更新 `docs/benchmarks.md`。

## 6. 風險與備註

- **三階 IIR 在低取樣率的數值穩定**：bilinear 在 Nyquist 附近有頻率壓縮；tone
  stack 轉角多在中低頻，影響小；必要時對關鍵轉角做 bilinear 預畸（prewarp）。
  係數推導全程 f64（見 §2.1）；若 f32 狀態在 96k 出現精度問題，單顆濾波器升
  f64 狀態的成本可忽略（每 slot 只有一個 stack）。
- **係數封閉式冗長**：Yeh 的 Bassman 係數式很長——正確性交給 §4.1 的 ngspice
  fixtures golden，不靠肉眼比對。
- **taper 與凹陷位置互相牽動**：若 noon 凹陷聽起來偏位，先查 taper 映射再懷疑
  元件值。
- **這是使用者最有感的一步**，且不依賴大框架——建議緊接 Phase 01 之後、或平行做。
</content>
