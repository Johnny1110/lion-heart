# PRD 023: 真實被動 tone stack 框架

狀態：**已實作（2026-07-27）— 待使用者耳朵驗收**
日期：2026-07-27
里程碑：Tone Revolution · Phase 02（`docs/tone_revolution/phase/02-tonestack-framework.md`）
關聯：PRD 011（`eq` 家族兩顆踏板）、ADR 003（drive model registry）、
PRD 022 / Phase 01（同計畫的前一步，彼此獨立）
新增 ADR：**030（tone stack 引擎）** — 引擎形式與遷移決策的來龍去脈在那裡

## 1. 背景與決策

現行 `drive::ToneStack` 是三個獨立相加的濾波帶——**圖形 EQ**。真實 Fender/Marshall
被動網路有兩件它沒有、卻是「像音箱」的關鍵：**旋鈕互動**與 **noon 天生的中頻凹陷**。
五顆 drive（`red-charlie`/`monster5150`/`angry-charlie`/`angry-charlie-v2`/`evva`）
都內建它，所以修好 tone stack 連帶修好一半的 drive 不滿。

被動 stack 是**線性**網路，不需要 `blocks::wdf`（那是為非線性 root 存在的）。

**與 Phase 02 初稿的一處實作偏離（重點，理由詳見 ADR 030）**：計畫寫的是「每種
拓撲手推一組封閉式係數 → bilinear 成三階直接式 IIR」。實際交付改為
**「netlist（資料）→ 數值解出連續狀態空間 → Tustin」**。同樣是「路線 A 線性網路」，
但：

- **穩定性成為結構性保證**（被動 RC 極點必為實負 → Tustin 必落單位圓內），初稿列為
  風險並準備以「級聯 + block-rate 解三次實根」當後備的問題直接消失；
- **係數變動天生連續**——狀態就是電容電壓（物理量），旋鈕動時重建係數、狀態沿用，
  這是 block-rate 重建不 click 的原因；
- **一個引擎吃所有拓撲**，加機型＝加一份 netlist，不動引擎——直接服務 Tone
  Revolution 目標 3（自研平台）。

## 2. 規格

### 2.1 `eq::tonestack` 引擎（新，`crates/lh-dsp/src/eq/tonestack.rs`）

- **Netlist as data**：`El::{Res, Pot, Cap}`，node 0 = GND、node 1 = IN 保留。
  `Pot` 帶 `knob` 索引與 `upper`（取 `f` 或 `1−f`），wiper 接觸電阻下限 1 Ω
  （真實存在，且讓 pot 打到底時 MNA 不奇異）。
- **netlist → (A,B,C,D)**：每顆電容 stamp 成「值等於自身狀態」的 MNA 電壓源；對
  `[x₁..x_k, u]` 的每個基底向量解同一個矩陣（一次消去、多個右手邊），讀出電容電流
  與輸出節點電壓即得 `(A,B,C,D)`。
- **Tustin**：`P=(I−½TA)⁻¹`、`A_d=P(I+½TA)`、`B_d=T·P·B`、`C_d=C·P`、
  `D_d=D+½T·C·P·B`；makeup 折進 `C_d`/`D_d`。
- **精度分工**：netlist 求解與離散全程 **f64**（節點導納跨五個數量級），逐 sample
  路徑 **f32**、≤4 個狀態。
- **RT 安全**：固定大小堆疊陣列（`MAX_NODES=8`、`MAX_CAPS=4`、MNA ≤12×12，~1.6 KB
  stack），零配置、零鎖；狀態 denormal flush；奇異 netlist 走 fallback 而非吐 NaN。
- **settled-skip**：旋鈕沒動就只做三個 float 比較（沿用 `eq::chain` 慣例）。
- **pot taper**：`Taper::{Linear, Audio, ReverseAudio}`，audio law 為半轉 10 %
  （`f(x) = (81^x − 1)/80`）。三個機型目前都 `Linear` — 理由見 §5。

### 2.2 機型註冊表 `KINDS`（append-only）

| Kind | 原型 | 元件值 | noon 凹陷 | makeup |
|---|---|---|---|---|
| `bassman` | Fender 5F6-A | slope 56k、treble 250k、bass 1M、mid 25k、C 250p/20n/20n | 9.5 dB | +7.38 dB |
| `jcm800` | Marshall 2203/2204 | slope 33k、treble 220k、bass 1M、mid 22k、C 470p/22n/22n | 7.4 dB | +5.32 dB |
| `big-muff` | EHX Big Muff tone | 39k/10n LP、3.9n/100k HP、100k pot | 隨旋鈕滑動 | +6.13 dB |

`bassman`/`jcm800` 共用同一個 `const fn fmv(...)`，只換零件。Bassman 的 mid 取
**原機 25k**，不取 BYOD 自行改過的 96k。

**`AC30` / `Baxandall` / `James` 暫緩**——無法可靠佐證其拓撲與元件值，不編造。
註冊表是純資料，補上任一個都不動引擎。詳見 ADR 030。

### 2.3 遷移（voicing 改動）

`red-charlie`/`monster5150`/`angry-charlie`/`angry-charlie-v2` → `jcm800`；
`evva` → `bassman`（家族唯一的 Fender 聲音）。**這五顆的聲音會變，是刻意的。**

drive 端的 `ToneStack` 變成薄包裝：`Drive` 已把旋鈕平滑成軌跡，stack 每
**64 sample** 取軌跡端點重建一次係數（`CHUNK` 是 256 = 5.3 ms，太粗會聽得到階梯；
64 sample ⇒ 750 Hz 更新率，與 `eq::chain` 同一個數字）。

### 2.4 獨立踏板 `eq::tonestack::Stack`

`eq` 家族 append 第三顆（Model / Bass / Mid / Treble / Level）。preset 以 key 記錄
選中踏板、plugin 由 `from_families` 展開 ⇒ **無 schema bump、既有 plugin 參數 id
不變**。GUI livery：漆面 tweed 棕（`TONESTACK`）——它是**音箱的** tone 網路，刻意
走出 eq 家族的冷色。Big Muff 機型真機只有一個旋鈕，blend 掛 Treble、Bass/Mid 無作用。

## 3. 驗收標準與實測

### 3.1 `cargo test`（全綠，lh-dsp 320 → 335 條）

**係數正確性 golden。** Phase 02 §4.1 要求 ngspice AC 掃描 fixtures；ngspice 不是
本專案的建置相依，**改以一支獨立的複數節點分析 oracle 取代**，寫在測試模組內、與
受測路徑**不共用任何程式碼**（電容 stamp 成 `sC` 導納、輸入節點移到右手邊；受測路徑
則是電容當狀態電壓源 + Tustin）。MNA stamp、電壓源正負號、電容電流讀取、Tustin 代數
任一環出錯，兩者就對不上。比 fixture 更好的地方是它涵蓋任意旋鈕點且跟著 CI 跑。

| 測試 | 標準 | 實測 |
| ---- | ---- | ---- |
| `discrete_response_matches_the_nodal_oracle` | 3 機型 × 7 組旋鈕（含極端）× 14 頻點 | 全數在容差內（0.15 dB + 隨頻率放寬到 0.6 dB @8 kHz，bilinear 預期的翹曲） |
| `the_mid_knob_moves_the_treble_response` | **旋鈕互動**：掃 Mid 改變 6.4 kHz | Bassman/JCM800 皆 >2 dB；Bass 掃動改 160 kHz 帶 >8 dB |
| `noon_has_the_signature_mid_scoop` | noon 於 400–800 Hz 相對 100 Hz/3.2 kHz 凹陷 | Bassman >8 dB、JCM800 >6 dB，且 **Bassman 比 JCM800 深 ≥1 dB** |
| `the_big_muff_notch_slides_with_its_knob` | 拓撲判別：凹陷位置隨旋鈕移動 | pos 4→8 由 ~1.3 kHz 滑到 ~400 Hz（>2×），noon 是內部極小非邊緣 |
| `the_mid_knob_lifts_the_scoop` | mid 0→10 抬升 800 Hz | >8 dB |
| `poles_stay_inside_the_unit_circle_at_every_rate` | 44.1/48/96 kHz × 全旋鈕掃描 | 全數 <1（冪迭代量測） |
| `noon_sits_near_unity_with_the_makeup_applied` | 每機型 noon 的頻帶平均 | 三者皆 \|avg\| < 0.5 dB |
| `sweeping_a_knob_is_click_free` | 掃旋鈕逐 block 重建 | max step < 0.25 |
| `settled_knobs_skip_the_rebuild` | settled-skip 生效 | 逐點驗證 |
| `every_knob_extreme_stays_bounded_and_finite` | 3×3×3 極端組合 | 有限、peak < 4 |
| `tapers_follow_their_law` / `taper_choice_moves_where_noon_lands` | taper 數學與其影響 | audio 半轉 = 0.1；換 taper 使 6.4 kHz 差 >3 dB |
| 多 rate / block、silence→silence、model 切換、踏板參數路由 | — | 全綠 |

**重新 pin 的既有 drive character 測試**（Phase 02 §2.3 要求）：

| 測試 | 改法與理由 |
| ---- | ---- |
| `{red_charlie,monster5150,angry_charlie,evva}_eq_bands_work` | 由「5→10 boost」改為**全行程 0→10**。被動 stack 只會**衰減**，noon 已接近 bass/treble 行程頂端，量 5→10 幾乎沒有量程——這是真機事實，不是缺陷。順帶把原本獨立的「mid 轉 0 = 金屬 scoop」併成全行程斷言的基準端 |
| `red_charlie_eq_bands_work`（新增段） | 加**耦合**斷言：Bass/Treble 固定、只轉 Middle，6.1 kHz/150 Hz 比值須變 >1.15×。**這一條在舊實作下不可能通過** |
| `{red_charlie,monster5150}_trims_lows_before_the_gain` | 主張是「**增益之前**的低頻修剪」，但 stack 在增益**之後**往反方向傾斜。改為把 stack 自身的 noon 傾斜（新增 helper `stack_tilt`）除回去——量的才是它宣稱的東西 |
| `red_charlie_cold_clipper_makes_even_harmonics` | 同理：JCM800 在 440 Hz 比 220 Hz 低 ~4 dB，h2/f0 改對 stack 傾斜比較，主張回到「削波器」本身 |
| `red_charlie_distorts_harder_than_the_screamer` | 探測基頻 220 → **330 Hz**：`harmonic_residual` 是整個諧波列對基頻的加權，在 220 Hz 上 stack 的凹陷會被誤讀成「失真較少」；330 Hz 處 stack 在前四個諧波內平坦到 2 dB |
| `evva_eq_is_flat_at_defaults` → `evva_noon_carries_the_bassman_scoop` | **斷言反轉**：evva 原本是家族唯一 noon 全平者，現在 noon 必須帶 Bassman 凹陷（>1.5×）。這個反轉本身就是本 PRD 的成果 |
| `modelled_pedals_sit_near_unity_at_default_knobs` | **未動、直接通過**——per-kind makeup 補掉插入損，如 §2.1 設計 |

### 3.2 `cargo bench -p lh-dsp`（同一次執行，數字入 `docs/benchmarks.md`）

| Bench | 實測 | 說明 |
| ----- | ---- | ---- |
| `eq_tonestack_settled` | ~1.58 µs | 與 `eq_parametric_4band`（1.49 µs）同級，遠低於 WDF |
| `eq_tonestack_knob_moving` | ~1.89 µs | 每 64 sample 重建一次 netlist ⇒ **+20 %**；單次重建 ~300 ns |
| `eq_3band`（對照） | ~0.65 µs | 舊的相加式三頻帶 |

遷移後的 drive 成本見 `docs/benchmarks.md` 的前後對照。

### 3.3 `assert_no_alloc`

重建路徑只有固定大小堆疊陣列（無 `Vec`、無 `Box`、無 `format!`），旋鈕掃描全程零
配置。既有 debug 路徑涵蓋。

### 3.4 耳朵（**待使用者驗收**）

- 同一顆 FMV 系 drive 前後 A/B：noon 是否有「音箱的凹陷骨架」；三旋鈕是否互動
  （轉 bass 感覺 treble 也變）；掃 Mid 是否聽到凹陷被抬起；整體像音箱不像圖形 EQ。
- 獨立 `tonestack` 踏板放 amp 前／後，Bassman vs JCM800 vs Big Muff 的辨識度。
- **`evva` 的改動最大**：它從「唯一 noon 全平」變成「帶 Bassman 凹陷」。若你覺得
  evva 該保留原本的乾淨 3-band，回復是一行——請回報。

## 4. 非目標

- 不做非線性 diode-ladder tone stack（需 Phase 03 的 R-Type）。
- 不追每台真機的元件公差。
- 不改 engine/session/plugin 訊息集，不 bump preset schema。
- 不移植 BYOD 的 WDF Bassman **程式碼**（GPL-3）。拓撲與元件值是事實；
  netlist→狀態空間→Tustin 的推導與實作是本專案自己的。

## 5. 已知取捨

- **taper 全設 `Linear`**：這些機型校準所對照的公開響應曲線（也是驗收 pin 住的 noon
  凹陷）都是線性軌半轉量的，而真機 taper 依年代與量測端而異。機制已實作並測試，
  改 voicing 是改一個欄位。若耳朵覺得凹陷位置偏了，**先查 taper 再懷疑元件值**。
- **每聲道各重建一次係數**：`Circuit::eq()` 是每聲道介面。旋鈕靜止時零成本；真的成
  瓶頸再把 stack 提到 `Drive` 層。
- **永遠不會 bit-transparent**：被動網路沒有「flat」設定，`eq` 家族另兩顆有的 flat
  快速路徑這顆沒有。這是實體事實。

## 6. 產出

- `crates/lh-dsp/src/eq/tonestack.rs`（新，引擎 + `KINDS` + 獨立踏板 + 26 條測試）
- `crates/lh-dsp/src/eq/mod.rs`：家族由 2 顆變 3 顆
- `crates/lh-dsp/src/drive/mod.rs`：`ToneStack` 改為引擎的薄包裝；character 測試重新
  pin；新增測試 helper `stack_gain`/`stack_tilt`
- `crates/lh-dsp/src/drive/{red_charlie,monster5150,angry_charlie,angry_charlie_v2,evva}.rs`：
  各自指定機型，刪掉不再使用的轉角頻率常數
- `app/lion-heart/src/gui/theme.rs`：`tonestack` livery
- `crates/lh-dsp/benches/effects.rs`：`eq_tonestack_settled` / `eq_tonestack_knob_moving`
- `docs/adr/030-tone-stack-engine.md`、`docs/benchmarks.md`
