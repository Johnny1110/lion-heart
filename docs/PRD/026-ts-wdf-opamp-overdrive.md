# PRD 026: `ts-wdf` — 完整 op-amp 回授式 Tube Screamer（Phase 04 第一顆）

狀態：**已實作（2026-07-28）— 待使用者耳朵驗收**
日期：2026-07-28
里程碑：Tone Revolution · Phase 04（`docs/tone_revolution/phase/04-opamp-overdrive-family.md`）
關聯：PRD 025 / ADR 032（WDF 可組合框架＝本顆的地基）、PRD 020 / ADR 028
（`screamer`）、PRD 021 / ADR 029（`sd1`）、PRD 022（omega root）
新增 ADR：**033（op-amp 模型參數政策 + 可選二極體 UX）**

## 1. 背景與決策

Phase 03 交付了框架（擁有式泛型樹 + R-Type + 有限增益 op-amp），但**沒有加任何
踏板**——刻意的，框架的正確性用「等價重寫既有兩顆」證明。本 PRD 是框架的第一個
**新**產物，也是 Phase 04 建議的起手式：TS 的元件值與拓撲逐行查核過，而且家族裡
已經有三個版本可以 A/B。

`ts-wdf` 把整個削波級當**一個電路**解：op-amp、回授網路、**位在回授路徑裡**的
二極體、gain leg、輸入耦合、負載，全部進同一棵 WDF 樹，每個過取樣 sample 解一次。

| 踏板 | 削波 | op-amp |
| --- | --- | --- |
| `ts9` | memoryless 曲線 | 一個增益數字 |
| `screamer` | WDF shunt 二極體 + 電容 | 一個增益數字 |
| `sd1` | WDF 二極體在迴路內 | **理想**（虛短） |
| **`ts-wdf`** | WDF 二極體在迴路內 | **有限增益 + 真實輸入/輸出阻抗** |

**為什麼拓撲重要**（這是白箱的整個賣點）：

- 增益是 `1 + Zf/Zg`，而 `Zg = R4 + 1/(sC3)`。~720 Hz 以下 `C3` 掐住回授電流、
  增益掉回 unity，低頻**乾淨地走過去**。招牌 mid-hump 不是誰在 clipper 外面加的
  濾波器，**是這個拓撲本身**。`screamer` 得手工做（gained path 高通 720 Hz + 乾訊
  相加）；這裡是長出來的。
- `C4`（51 pF）跨在回授電阻上，轉角 `1/(2π·Rf·C4)` 而 `Rf` **就是 drive 電位器**：
  drive 0 時 61 kHz、drive 10 時 5.7 kHz。**轉大真的變暗。**
- 二極體面對的是回授節點而非訊號節點，所以它們對抗的是一個**頻率相依的阻抗**——
  削波門檻不是一個數字。

## 2. 規格

### 2.1 Netlist（TS-808/TS-9 削波級 IC1B）

節點 0=地、1=`+`、2=`−`、3=輸出、4=op-amp 內部。

```
els   : op_amp(1, 2, 3, 4, AG, RI, RO)
ports : [ (3,2) up=回授(含二極體) , (1,0) 輸入腳 , (2,0) gain leg , (3,0) 負載 ]
```

元件：`R5=10k`、`C2=1µF`、`R4=4.7k`、`C3=0.047µF`、`R6=51k`、`Pot1=500k`、
`C4=51pF`、`RL=1M`。drive 律 `Rf = 51k + 500k·n²`（audio taper），與 `ts9`/
`screamer` **同一條**，所以四顆的「drive 6」意思相同。

樹：

```rust
type InputLeg  = Parallel<CapacitiveVoltageSource, Resistor>;               // C2(載 Vin) ‖ R5
type OpAmpNode = RType<4, 3, (InputLeg, ResistorCapacitorSeries, Resistor)>; // 上圖 junction
type ClipTree  = Parallel<ResistorCapacitorParallel, OpAmpNode>;            // (R6+pot)‖C4  ‖  R-node
```
root = `DiodePair`（omega 閉式，PRD 022）。

**up port 放在回授路徑**（輸出→反相輸入）。那是二極體掛的地方，也是高阻抗點
（audio 頻段約 `(AG+1)·Zg` ≈ 14 MΩ），所以 `R_up = R_戴維南` 的 adapted 條件良態
——不像 op-amp 輸出腳，被回授壓到毫歐姆（ADR 032 §5 踩過的坑）。

### 2.2 對參考模型的兩處**刻意偏離**（詳見 ADR 033）

1. **op-amp 參數用 JRC4558 datasheet，不沿用參考實作的 `(Ag=100, Ri=1e9,
   Ro=0.1)`。** 本專案採 `AG=3000`、`RI=5e6`、`RO=75`。`Ag=100` 大約是 4558 開迴路
   增益掉到 **30 kHz** 的值；用在整個吉他頻段會**同時壓掉本顆的兩個招牌**——drive
   掃程頂端被壓平（drive 10 要 117× 只給 54×）、`C4` 的高頻衰減被糊掉（實測
   `cranked/open` 只有 0.77，計畫 §4.1 要求「drive 轉大變暗」）。
2. **二極體選單帶 `(Is, n)` 兩個參數，不只 `Is`。** 參考實作的選單只有 `Is`，把
   ideality 折進使用者面板的「# Diodes」旋鈕，並給 `1N34 → 200 pA`。配上矽的 `n`，
   那個值會讓**鍺**檔位削得比矽**還高**——與它命名的零件相反，而且比流通的 1N34A
   SPICE 模型（`IS=2.0e-7`）小 1000 倍，像是 nano/pico 的單位滑手。

### 2.3 面板：Drive / Diode / Count / Tone / Level

- **Diode**（stepped）：`1N4148`（原廠，pair-level 擬合 4.352 nA / n 1.906）、
  `GZ34`（一般小訊號矽 2.52 nA / 1.75）、`1N34`（鍺 200 nA / 1.28，約半個 clamp）、
  `LED`（紅光 LED，~1.5 V@1 mA 的量級擬合，非 datasheet 萃取）。
- **Count**（連續 0.3–3.0，預設 1.0）：每支路串幾顆。連續是因為它縮放的量
  （`m·n·Vt`）本來就連續；1.5 不是一顆半二極體，是介於一顆與兩顆之間的轉角。
- Tone 級**刻意與 `screamer` 完全相同**（723 Hz one-pole tilt + makeup + DC
  block），如此兩顆的 A/B 就純粹是削波模型的比較。

實作路由（計畫 §2.7 預期的「純內部改動，零 schema 影響」）：`Ctl::Shape` 走既有的
stepped 路徑；新增 `Ctl::Trim` + `Circuit::set_trim` 預設 no-op hook 給 Count。
兩者都是**新 key 才有的參數**，既有踏板一個都沒動。

### 2.4 旋鈕與重建時機

`shape()` 以 **64 個過取樣 sample** 為子區塊；每個子區塊邊界做一次
`retune()`：drive 動了就 `calc_impedance()`（settled 時一次 float 比較就跳過），
Count 則以 ~10 ms 一階 glide 走向目標（切二極體**型別**是 stepped，就讓它 step）。
熱迴圈內零阻抗運算、零矩陣重建。

## 3. 驗收標準與實測

### 3.1 `cargo test`（lh-dsp 365 → **378** 條，workspace 全綠）

**最強的一條**：`the_linear_response_matches_hand_solved_ac_analysis`。二極體轉角
以下整級是線性的，轉移函數是教科書的，所以 WDF 的實測響應必須對得上**同一份
netlist 的手解 AC 分析**（`H_in · Ag/(1+Ag·β)`，`β = Zg/(Zg+Zf)`，`Zf` 含二極體
小訊號電阻 `nVt/2Is`，並做雙線性預扭）。

這條**與實作零共用推理**——沒有波變數、沒有散射矩陣、沒有 adaptor 代數、沒有樹。
junction 接錯、port 對調、受控源 stamp 反向、up port 掛錯節點，都會在這裡爆掉。
**3 個 drive 位置 × 5 個頻率，全數 < 2 %。**

| 測試 | 標準 | 實測 |
| ---- | ---- | ---- |
| `the_linear_response_matches_hand_solved_ac_analysis` | 對照手解 AC 分析 | 15 組全數 < 2 % |
| `the_diode_root_is_solved_to_tolerance` | `a = v + R·i(v)`，掃幅掃頻 50 000 sample | 最差相對殘差 < 1e-3 |
| `the_mid_hump_comes_from_the_gain_leg` | 1 kHz 增益 > 3× 的 100 Hz，且低頻增益 < 8× | 通過（線性區量測，純拓撲） |
| `turning_up_the_drive_darkens_the_stage` | drive 10 的 8k/1k 傾斜 < 0.7× drive 1 的 | 通過 |
| `break_up_is_band_limited_from_both_ends` | 中頻破音 > 低頻**且** > 高頻各 1.2× | 通過（120 Hz / 1 kHz / 12 kHz） |
| `the_shortfall_from_ideal_grows_with_demanded_gain` | 實測必低於理想 `1+Zf/Zg`，且落差隨要求增益成長 > 3× | 通過 |
| `the_diode_selector_moves_the_knee_the_right_way` | 鍺 < 0.75× 矽 < LED/1.5 | 通過 |
| `stacking_diodes_raises_the_clamp` / `the_count_knob_glides_instead_of_stepping` | 疊高 clamp；且 glide 不是 step | 通過 |
| `the_response_holds_across_sample_rates` | 44.1/48/96 kHz 對同一份 AC 分析 | 全數 < 2 % |
| `silence_stays_silent` / `bounded_when_slammed` | 靜音出精確 0；±1e6 狂推有界有限 | 通過 |

`break_up_is_band_limited_from_both_ends` 是**白箱判別**，而且是兩側的：低頻由
`C3` 擋、高頻由會移動的 `C4` 擋，所以同一個輸入電平下只有中頻被削平。一條曲線配
一個濾波器可以偽造其中一側；兩側都偽造、而且轉角還跟著 drive 旋鈕跑，那就等於
把這個電路建出來。

**家族層級**：`every_model_*` 系列（有界／DC 阻擋／靜音／諧波／多 rate／
模型切換）自動涵蓋新踏板；`registry_is_consistent`、theme distinct-livery 亦然。

**新增的離線 RT 閘門**：`crates/lh-dsp/tests/alloc.rs`——獨立測試 binary，裝上
`AllocDisabler` 當 global allocator，把**全部 16 顆**踏板在掃所有旋鈕（含 stepped
選擇器）＋ 1e5 狂推下跑過 `process`。此前這條規則只有 app 的 debug build 在
**執行期**擋。已用故意注入的 `vec![]` 確認它會 SIGABRT——不是一條不會紅的測試。

### 3.2 `cargo bench -p lh-dsp`（同一輪，Linux 開發容器）

| Bench | 中位數 | 讀法 |
| ----- | ------ | ---- |
| `drive_ts9_4x_oversampled` | ~17.9 µs | memoryless 參照 |
| `drive_screamer_4x_oversampled` | ~31.9 µs | shunt WDF |
| **`drive_ts-wdf_4x_oversampled`** | **~42.1 µs** | 本顆，旋鈕靜止 |
| **`drive_ts-wdf_knob_sweeping`** | **~48.7 µs** | 本顆，drive 旋鈕持續轉動 |
| `drive_sd1_4x_oversampled` | ~71.7 µs | 理想虛短 WDF |

42.1 µs 是 1,333 µs 預算的 **3.2 %**，落在 `screamer` 與 `sd1` 之間——多的是
R-Type 的 4×4 矩陣-向量乘與多一層 adaptor。**旋鈕轉動時 +16 %**（8 次全樹重建／
block ≈ 840 ns 一次），靜止時為零：這就是 ADR 032「執行期數值散射矩陣」在一顆真實
電路上的完整代價，先前只在合成 junction 上量過。

`drive_screamer` 本輪 31.9 µs vs PRD 025 那輪 31.8 µs——機器狀態一致，上表可直接
互比。

### 3.3 電平校準

`+1.03 dB`（預設旋鈕、220 Hz 吉他訊號），家族中位附近（`screamer` +0.86、
`sd1` −0.08、`ts9` +2.07）。`MAKEUP = 0.30`。新增 `default_level_survey`
（`#[ignore]` 診斷）印出全家族的預設電平，供本 Phase 後續五顆校準用。

### 3.4 耳朵（**待使用者驗收**）

四顆 Screamer 的 A/B 是本顆存在的理由：`ts9`（memoryless）／`screamer`（shunt）／
`sd1`（理想虛短）／`ts-wdf`（完整拓撲 + 有限增益）。聽點：mid-hump 的自然度、
drive 轉大是否變暗、Diode 切鍺/LED 的手感差。

## 4. 非目標

- 不做整顆踏板每一級（電源／旁通／緩衝／輸出 tone 網路的完整被動解）。
- 不追 SPICE 位元對拍。
- 不動 `ts9`/`screamer`/`sd1` 的任何行為。
- 不抄任何已發表的散射矩陣（ADR 032 起，矩陣一律由本專案的 junction netlist 在
  執行期數值構造）。

## 5. 已知取捨

- **常數 `AG` 不可能到處都對**：真實 op-amp 增益以 6 dB/oct 下滑，而 R-Type
  junction 內部不能放電抗元件（ADR 032）。最高一個八度因此被模擬成比實物多的迴路
  增益。要修得靠 junction 支援電抗內部元件，或把主極點拉到 junction 外面當一個
  port——都留給後續。
- **`LED` 的 `(Is, n)` 是量級擬合**，非 datasheet 萃取；已在程式碼註解標明。
- **`ts-wdf` 的面板一旦出貨就定了**（append-only 規則：既有 key 不得加參數），
  所以 Diode/Count 是**現在**就得決定的事，不是之後能補的。
- 旋鈕轉動時 +16 % CPU（見 §3.2）。

## 6. 產出

- `crates/lh-dsp/src/drive/ts_wdf.rs`（新）
- `crates/lh-dsp/src/blocks/wdf/rtype.rs`：`RType::port_voltage`（輸出節點取樣）
- `crates/lh-dsp/src/blocks/wdf/diode.rs`：`DiodePair::set_params`（執行期換二極體）
- `crates/lh-dsp/src/drive/mod.rs`：`Ctl::Trim` + `Circuit::set_trim`、registry 追加、
  `model_of()` 測試輔助（測試不再以位置硬編碼踏板）、`default_level_survey`
- `crates/lh-core/src/preset.rs`：`DRIVE_PEDALS` 追加 `ts-wdf`
- `app/lion-heart/src/gui/theme.rs`：TS-808 綠 livery
- `crates/lh-dsp/tests/alloc.rs`（新）、`crates/lh-dsp/benches/effects.rs`
- `docs/adr/033-opamp-model-and-diode-ux.md`、`docs/benchmarks.md`
