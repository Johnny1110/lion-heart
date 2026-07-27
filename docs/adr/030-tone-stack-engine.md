# ADR 030: Tone stack 引擎 — netlist → 狀態空間，取代相加式三頻帶

狀態：**已採納（2026-07-27）**
關聯：`docs/tone_revolution/phase/02-tonestack-framework.md`（本 ADR 的來源計畫）、
PRD 023（落地規格）、ADR 003（drive model registry）、PRD 011（`eq` 家族）
取代：`drive::ToneStack`（相加式三頻帶）之聲學行為——**非 append-only，voicing 改動**

## Context

`drive::ToneStack` 是三個**互不干擾、相加**的濾波帶：

```rust
*s = x + lo_gain*lo + mid_gain*bp + hi_gain*hi;
```

這是圖形 EQ，不是 tone stack。真實 Fender/Marshall 被動網路的兩個決定性性質它
都沒有：

1. **旋鈕互動**——三個電位器負載同一組節點，轉 Mid 會改變 treble 響應；
2. **noon 天生不平**——招牌中頻凹陷（Bassman ~9.5 dB、JCM800 ~7.4 dB）加上實打實
   的插入損。

使用者的回饋「聽起來像 hi-fi EQ，不像音箱」直指這兩點；而 `red-charlie` /
`monster5150` / `angry-charlie` / `angry-charlie-v2` / `evva` 五顆 drive 全都內建
這個 stack，所以修好它連帶修好一半的 drive 不滿。

被動 tone stack 是**線性**網路（純 R/C/pot），所以不需要 `blocks::wdf`——那是為
非線性 root 存在的。Phase 02 計畫據此拍板「路線 A：解析傳輸函數」，具體做法寫的是
**每種拓撲手推一組封閉式係數 + bilinear 成三階直接式 IIR（TDF2）**。

## Decision

採用路線 A 的**線性網路**方向，但實作形式與 Phase 02 初稿不同，改為
**「netlist（資料）→ 數值求解出連續狀態空間 → Tustin 離散」**：

```
netlist ─(MNA，電容當狀態電壓源)→ (A,B,C,D) ─(Tustin)→ (Ad,Bd,Cd,Dd) → 逐 sample 跑
```

1. 每個機型是一份 **netlist**（`El::{Res, Pot, Cap}` 的 `&'static [El]`），不是一組
   手推係數式。
2. **狀態＝電容電壓**。每顆電容以 MNA stamp 成「值等於自身狀態」的電壓源；對
   `[x₁..x_k, u]` 的每個基底向量各解一次同一個矩陣，就直接讀出全部電容電流與輸出
   節點電壓——那就是 `(A, B, C, D)`。
3. Tustin：`P = (I − ½TA)⁻¹`、`A_d = P(I + ½TA)`、`B_d = T·P·B`、`C_d = C·P`、
   `D_d = D + ½T·C·P·B`。機型的 makeup 增益折進 `C_d`/`D_d`。
4. 全部在 **block 邊界、f64** 執行，且**只在旋鈕真的動過時**（settled-skip，沿用
   `eq::chain` 慣例）；逐 sample 路徑是 f32、≤4 個狀態。

### 為何偏離初稿的「手推封閉式 + 直接式 IIR」

三個理由，兩個正是初稿自己列為風險的項目：

- **穩定性變成結構性保證，而非需要測出來的性質。** 被動 RC 網路的極點必為實負，
  Tustin 必然把它們映進單位圓內——任何旋鈕位置、任何取樣率皆然。初稿擔心三階直接
  式的數值穩定，並把「必要時升級成級聯」列為後備（代價是 block-rate 解三次實根）；
  狀態空間**不需要求根就得到級聯的良態**。
- **係數變動天生連續。** 狀態就是電容電壓，是**物理量**；旋鈕移動時重算係數，狀態
  沿用即可，不像直接式 IIR 的抽象記憶被重新詮釋。這是 block-rate 重建不會 click
  的原因，也讓「係數平滑」退化成「旋鈕平滑」一層。
- **一個引擎吃所有拓撲，加機型＝加一份 netlist。** 初稿要求「每種拓撲一組封閉式」
  （FMV 一式、Baxandall 一式、Big Muff 一式…），每一式都是一次冗長且易錯的手推
  代數。改成數值求解後，FMV 與 Big Muff 這兩種**完全不同**的拓撲共用同一段程式，
  且直接服務 Tone Revolution 目標 3（自研踏板平台）：使用者加一顆 tone 網路只需
  寫 netlist。

代價是每次重建要解一個 ≤12×12 的線性系統而非套 30 個乘加。實測（見
`docs/benchmarks.md`）：settled 1.58 µs / 64-frame block，旋鈕狂轉（每 64 sample
重建一次）1.89 µs——**+20 %**，與 `eq_parametric_4band` 同級。這個代價買下上面三項，
划算。

### 機型註冊表

`KINDS`（append-only）目前三個：

| Kind | 拓撲 | 元件值來源 | noon 凹陷 | makeup |
|---|---|---|---|---|
| `bassman` | FMV | Fender 5F6-A：slope 56k、treble 250k、bass 1M、mid 25k、C 250p/20n/20n | 9.5 dB | +7.38 dB |
| `jcm800` | FMV | Marshall 2203/2204：slope 33k、treble 220k、bass 1M、mid 22k、C 470p/22n/22n | 7.4 dB | +5.32 dB |
| `big-muff` | LP/HP blend | EHX Big Muff：39k/10n LP、3.9n/100k HP、100k pot | 隨旋鈕滑動 | +6.13 dB |

`bassman` 與 `jcm800` 是**同一份 `const fn fmv(...)`、只換零件**——「一個引擎、換
零件＝多機型」在程式碼層面成立。

**Bassman 的 mid 取原機 25k**，不取 BYOD `BassmanToneStack.h` 的 96k（其原始碼註解
自承 `modified from 25e3`，是它自己的 voicing 選擇）。

### 未落地的機型（誠實界定）

Phase 02 §2.2 列了六個機型，本次交付三個。**`AC30`、`Baxandall`、`James`/`Passive`
暫緩**，理由是同一個：**我無法可靠佐證它們的拓撲與元件值**。

- `AC30`：Top Boost 的 tone 網路我只能猜。實測把 FMV 的 mid leg 固定成 10k 會得到
  **比 Bassman 更深**的凹陷（12.2 dB），與 Vox「mid 前傾、不凹」的公認特徵矛盾——
  這是我猜錯了值，不是 Vox 真的如此。
- `Baxandall` / `James`：主動 Baxandall 的回授網路與被動 James 的接法，我沒有可靠
  來源，硬寫等於編造元件值。

註冊表是 append-only 的**純資料**，補上任何一個都是加一份 netlist + 一列表格，
**不動引擎**。等有 schematic 再補。

Phase 02 §4.1 原以 `Baxandall`「noon 近平坦」當對照組；改由**跨機型判別**取代：
`bassman` 的凹陷必須比 `jcm800` 深 ≥1 dB（同拓撲、只差零件），而 `big-muff` 的凹陷
**位置隨旋鈕滑動** 而 FMV 的不會（不同拓撲）。

### Pot taper

`Taper::{Linear, Audio, ReverseAudio}` 已實作並測試（audio law：半轉 10 %）。
**三個機型目前都設 `Linear`**：這些機型校準所對照的公開響應曲線（也是驗收測試 pin
住的 noon 凹陷）都是線性軌半轉量的，而真機的 taper 依年代與量測端而異。改 voicing
是改一個欄位，機制已就位。

### 遷移（voicing 改動）

五顆 drive 直接遷移，採 Phase 02 §2.3 的建議 (i)：

| 踏板 | 機型 | 理由 |
|---|---|---|
| `red-charlie` | `jcm800` | 就是 2203 |
| `monster5150` | `jcm800` | Marshall 衍生的 5150 |
| `angry-charlie` | `jcm800` | JHS Angry Charlie ＝盒裝 JCM800 |
| `angry-charlie-v2` | `jcm800` | 同上 |
| `evva` | `bassman` | 自家設計，給家族唯一的 Fender 聲音 |

**這會改變這五顆的聲音，是刻意的。** v0.1.0 已釋出，但本專案目前單人使用、preset
皆在本機——依 `docs/tone_revolution/overview.md` §7 的共同決策，voicing 改動以
「ADR 交代 + 測試重新 pin」處理，不背相容性包袱。

`evva` 的改動最大：它原本是家族裡唯一 noon 全平的 3-band，現在 noon 帶 Bassman
凹陷。舊測試 `evva_eq_is_flat_at_defaults` 改名 `evva_noon_carries_the_bassman_scoop`
並**反轉**其斷言——那個反轉本身就是本 ADR 的成果。若使用者耳朵覺得 evva 該保留原本
的平坦 EQ，改回去是一行（`ToneStack::new(kind::BASSMAN)` → 舊的相加式），或依 §2.3
的選項 (ii) 以新 key 追加變體。

### 獨立踏板

`eq` 家族 append 第三顆 `tonestack`（Model / Bass / Mid / Treble / Level）。preset
以 **key** 記錄選中的踏板，plugin 由 `from_families` 自動展開——**無 schema bump、
既有 plugin 參數 id 不變**。Big Muff 機型真機只有一個旋鈕，其 blend 掛在 Treble，
Bass/Mid 對它無作用（`Kind::uses_knob` 是那份事實的程式化）。

## Consequences

**好的**

- 五顆 FMV 系 drive 現在有旋鈕互動與 noon 凹陷；測試可量測地證明兩者
  （`red_charlie_eq_bands_work` 的耦合斷言在舊實作下**不可能**通過）。
- 加機型 = 加 netlist，不動引擎；直接支撐 Tone Revolution 目標 3。
- 極點穩定性是定理不是運氣；`poles_stay_inside_the_unit_circle_at_every_rate`
  以 Jury 式的冪迭代在 44.1/48/96 kHz × 全旋鈕掃描確認。
- 沒有引入 `blocks::wdf` 依賴，Phase 03 的 WDF 框架與本引擎正交。

**要付的**

- 每次重建一個 ≤12×12 f64 線性解（~300 ns）。旋鈕靜止時完全跳過。
- MNA 用固定大小堆疊陣列（`MAX_NODES=8`、`MAX_CAPS=4`、~1.6 KB stack），**netlist
  規模有上限**。超過就要調常數（會被 `knob_masks_match_the_netlists` 的
  `debug_assert` 擋下）。
- 被動 stack 永遠不會 bit-transparent，也沒有「flat」設定——這是實體事實，不是缺陷，
  但 `eq` 家族的另兩顆踏板有 flat 快速路徑而這顆沒有。
- drive 的 `Circuit::eq()` 是**每聲道**介面，所以左右各自重建一次係數（重建成本
  ×2）。旋鈕靜止時無成本；真的成為瓶頸再把 stack 提到 `Drive` 層。

**沒做的**

- 非線性 diode-ladder tone stack——需要 Phase 03 的 R-Type，本 ADR 純線性。
- 不追每台真機的元件公差。
- 沒有移植 BYOD 的 WDF Bassman 程式碼（GPL-3）。拓撲與元件值是**事實**；
  netlist→狀態空間→Tustin 的推導與實作是本專案自己的。
