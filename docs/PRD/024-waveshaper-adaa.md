# PRD 024: Waveshaper bank + ADAA 抗鋸齒

狀態：**已實作（2026-07-27）— 待使用者耳朵驗收**
日期：2026-07-27
里程碑：Tone Revolution · Phase 06（`docs/tone_revolution/phase/06-waveshaper-adaa.md`）
關聯：ADR 003（drive registry）、PRD 023 / ADR 030（tone stack，同一版一起落地）
新增 ADR：**031（ADAA 抗鋸齒策略）** — 階數選擇、f64 決定、dry-sum 分析在那裡

## 1. 背景與決策

兩個獨立但同源的主題：

1. **抗鋸齒（品質，全家族受益）。** memoryless drive 走「4× 過取樣 + 逐點取樣」，
   而硬轉角的諧波衰減太慢，4× 擋不住。實測非諧波地板：六顆踏板落在 **−28 ～
   −30 dB**——那就是「高把位毛躁、沙沙」。這與 tone stack（PRD 023）並列為
   「drive 不滿意」的兩個根源。
2. **創意波形整形 bank。** 一顆新踏板，承載 lion-heart 沒有的「數位/合成器味」
   調色盤。

交付三件：(a) 可重用的 ADAA 基礎設施、(b) 家族**每一個** memoryless 削波級接上
去、(c) 新的 `waveshaper` 踏板。

**與 Phase 06 計畫的偏離**（詳見 ADR 031）：二階 ADAA 由**自行從定義推導**，得到
兩半各自獨立退化的對稱形式，退化分支是單次 `f` 求值而非巢狀 case。

## 2. 規格

### 2.1 `blocks::waveshaper`（新）

- **`Adaa1`**：`y = (F₁(x₀) − F₁(x₁)) / (x₀ − x₁)`，一個 sample 狀態、半 sample 延遲。
- **`Adaa2`**：`y = A(Δ₀) + A(Δ₂)`，`A(Δ) = (F₂(x₁+Δ) − F₂(x₁))/Δ² − F₁(x₁)/Δ`；
  `Δ → 0` 時退回 `½·f(x₁ + Δ/3)`。兩 sample 狀態、一 sample 延遲。
- **精度**：ADAA 算術與曲線全程 **f64**（差商的抵銷不給 f32 留餘地）；f32 只在
  進出口。所有 `F₁`/`F₂` 正規化成 `F(0) = 0`——`reset` 依賴這點，且讓原點附近被
  相減的數值保持小。
- **熱路徑優化**：`tanh_f1`（`ln cosh`）在 `|x| > 20` 提前返回 `|x| − ln 2`。修正項
  小於 `|x|` 的一個 f64 ulp，**不是近似，是同一個數字**，省掉兩個超越函數。
- **RT 安全**：純函數 + 少量狀態，零配置；狀態 denormal flush；退化分支有界。
- **Shape bank**（12 條，append-only）：`Soft`(tanh)、`Hard`(clamp)、`Asym`、
  `Diode`、`Sine`、`Fold`(三角摺疊)、`Digital`(量化階梯)、`Cheby2..5`、`Fuzz`。
  有初等 `F₂` 者（Hard/Asym/Diode/Sine/Fold/Fuzz）走二階，其餘一階。

### 2.2 既有 drive 抗鋸齒改造

家族裡**每一個** memoryless 削波級都接上 ADAA，共 12 顆。`screamer`/`sd1` 不接
（WDF，非線性藏在解出來的方程式裡，沒有 `F₁` 可談）。

階數：硬切走二階（`angry-charlie`、`angry-charlie-v2`、`monster5150`、`ts9` 的代數
曲線）；`tanh` 系走一階（`bd2`、`evva`、`red-charlie`、`jan-ray`、`classic`、
`fuzz-face`、`centaur`、`overdrive`）。

**dry-sum 對齊：不需要。** 計畫 §2.3 把「ADAA 的半 sample 延遲與未延遲 dry 路徑
相加會梳狀」列為最容易踩的坑，備了三套補償方案。實測後**一套都不需要**——因為
ADAA 跑在 4× 率上，折算基頻只有 0.125 sample：最壞漣漪 1 kHz 0.01 dB、10 kHz
0.09 dB、16 kHz 0.22 dB。數字 pin 在 `dry_sum_comb_error_is_small_at_4x`，並註明
**若過取樣倍率下修，這個結論要重驗**。

### 2.3 新踏板 `waveshaper`

Drive / Shape（stepped，12 條曲線）/ Tone（700 Hz–14 kHz post LP）/ Level。
家族裡唯一不模擬電路的一顆。切 Shape 會清 ADAA 狀態（它存的是**離開那條曲線**的
`F₁`）。

Shape 是 stepped 選擇器，需要新的 `Ctl::Shape` 路由——直接送到 circuit 而非經過
平滑器。`Ctl` 是 `lh-dsp` 私有型別，**零 preset/plugin schema 影響**；
`DRIVE_PEDALS` append 到 15，plugin 由 registry 展開，既有參數 id 不變。
GUI livery：實驗室青（`WAVESHAPER`）。

## 3. 驗收標準與實測

### 3.1 `cargo test`（lh-dsp 332 → 341 條，全綠）

**ADAA 正確性**（不靠推導背書）：

| 測試 | 標準 | 實測 |
| ---- | ---- | ---- |
| `a_linear_curve_reproduces_the_kernels` | 線性 `f` 下一階＝兩點平均、二階＝`(x₀+4x₁+x₂)/6` | 通過（這條定住核的形狀） |
| `adaa_matches_the_kernel_it_claims_to_implement` | 12 曲線 × 兩階數 × 6 組取樣點，對照**定義積分的數值求解** | 全數 < 2e-4 |
| `antiderivatives_differentiate_back_to_the_curve` | 中央差分驗 `F₁′ = f`、`F₂′ = F₁`，並驗 `F(0) = 0` | 全數 < 1e-4 |
| `a_held_input_falls_back_cleanly` / `tiny_steps_stay_smooth` | 退化分支不除零、不跳、收斂到曲線本身 | 通過（含 1e-7 步進穿過轉角） |

**抗鋸齒地板**：

| 測試 | 標準 | 實測 |
| ---- | ---- | ---- |
| `adaa_lowers_the_alias_floor_under_the_same_oversampling` | 同曲線、同 4× 過取樣，只差 ADAA 開關 | `Hard` > 25 dB、`Fold` > 15 dB、驅到平頂的 `Soft` > 10 dB |
| `every_memoryless_clipper_is_anti_aliased` | 15 顆逐一 pin 住地板 | 見下表 |

| 踏板 | 前 | 後 | 踏板 | 前 | 後 |
|---|---|---|---|---|---|
| `ts9` | −37.4 | **−86.9** | `bd2` | −29.9 | **−56.7** |
| `angry-charlie` | −28.6 | **−78.8** | `overdrive` | −29.8 | **−54.7** |
| `centaur` | −50.7 | **−78.3** | `fuzz-face` | −29.4 | **−52.7** |
| `jan-ray` | −41.2 | **−68.1** | `angry-charlie-v2` | −29.5 | **−42.7** |
| `classic` | −31.5 | **−63.2** | `red-charlie` | −29.3 | **−39.6** |
| `evva` | −34.0 | **−59.1** | `monster5150` | −29.5 | **−38.0** |

> 串級的收益打折是**原理上的**：ADAA 假設輸入在 sample 之間是直線，這對進入
> **第一個**削波器的訊號成立，對進入第二個的已方波化訊號不成立。三級的
> `monster5150` 拿 8.5 dB，單級的 `angry-charlie` 拿 50 dB。

**既有 character pin**：**一條都沒改、全部通過**，含 level-norm
（`modelled_pedals_sit_near_unity_at_default_knobs`）。計畫 §2.3 預期可能要重新
pin 並記錄微幅 voicing 變動——沒有發生。

**新踏板**：`waveshaper_every_curve_reaches_the_audio`（每條曲線相對預設曲線的
渲染差異 > 0.5 %）、`waveshaper_shape_switch_is_bounded_and_stereo_locked`、
`no_two_curves_are_the_same_function`（函數層級的精確判別）、
`chebyshev_curves_generate_their_own_harmonic`、
`the_wavefolder_folds_more_as_drive_rises`。

### 3.2 `cargo bench -p lh-dsp`

**隔離量測**（同一條曲線、同一條 4× 過取樣鏈，只差 ADAA）：

| Bench | 中位數 | 相對 |
| ----- | ------ | ---- |
| `waveshaper_hard_plain` | ~4.2 µs | — |
| `waveshaper_hard_adaa1` | ~4.5 µs | **+7 %** |
| `waveshaper_hard_adaa2` | ~5.4 µs | **+28 %** |

整顆踏板的漲幅更大（`tanh` 的 `ln cosh` 是 f64 超越函數，串級踏板有兩三個）。
最貴的 `red-charlie` 在本容器 ~33 µs = deadline 的 2.5 %，仍在預算內。
**本次容器比 PRD 022 那輪慢約 1.8 倍**（未改動的 `screamer` 量到 54.7 µs vs
記錄的 30.5 µs），所以跨場次的絕對值不可比——見 `docs/benchmarks.md` 的說明。

### 3.3 `assert_no_alloc`

ADAA 是純函數 + 固定狀態，零配置；`waveshaper` 選取、切曲線、狂推全程無配置。

### 3.4 耳朵（**待使用者驗收**）

- **高把位單音與和弦，改造前後 A/B**：高頻「毛躁/沙沙」應明顯減少，而 voicing
  不變（測試說不變，耳朵是最後一關）。`angry-charlie` 與 `ts9` 差最多（50 dB），
  `monster5150` 與 `red-charlie` 差最少（8–10 dB）。
- **`waveshaper` 掃 Shape**：`Fold` 的 West Coast 摺疊味、`Cheby3/5` 的純諧波、
  `Digital` 的 lo-fi 階梯、`Fuzz` 的方波閘門。

## 4. 非目標

- 不搬 Surge/BYOD 的 waveshaper 程式碼（GPL-3）——曲線是公開數學，反導數與 ADAA
  推導是本專案自己算的。
- 不改 voicing——抗鋸齒是「同曲線、少鋸齒」。
- 不做 WDF——本 PRD 純 memoryless。
- 不追完全零混疊；不降過取樣倍率換 ADAA（見 ADR 031）。

## 5. 已知取捨

- **f64 進了熱迴圈**。與 PRD 022（WDF root 從 f64 降到 f32 近似求速度）方向相反，
  理由不同：差商的抵銷不給 f32 留餘地。
- **曲線與反導數必須成對維護**，且正規化成 `F(0) = 0`。中央差分測試把關。
- **串級削波的收益有上限**（見上）。要再進一步得整條串級一起做連續時間處理。
- **`Digital` 的量化步階從 1/24 改成 1/6**：`no_two_curves_are_the_same_function`
  抓到 1/24 的階梯與純硬切處處相差不到 0.017——那是聽不見的 bit-crush。

## 6. 產出

- `crates/lh-dsp/src/blocks/waveshaper.rs`（新：ADAA1/2 + 12 條曲線與反導數 + 16 條測試）
- `crates/lh-dsp/src/drive/waveshaper.rs`（新踏板）
- `crates/lh-dsp/src/drive/mod.rs`：`Ctl::Shape`、`Circuit::set_shape`、registry
  append、地板 pin 測試 + 新踏板 character 測試
- 12 顆既有 drive 的削波級接 ADAA
- `crates/lh-core/src/preset.rs`：`DRIVE_PEDALS` → 15
- `app/lion-heart/src/gui/theme.rs`：`waveshaper` livery
- `crates/lh-dsp/benches/effects.rs`：`bench_adaa` 群組
- `docs/adr/031-adaa-anti-aliasing.md`、`docs/benchmarks.md`
