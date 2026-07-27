# PRD 022: 閉式非線性 root — Wright Omega 取代 Newton 迭代

狀態：**已實作（2026-07-27）— 待使用者耳朵驗收**
日期：2026-07-27
里程碑：Tone Revolution · Phase 01（`docs/tone_revolution/phase/01-fast-nonlinear-root.md`）
關聯：PRD 020 / ADR 028（`blocks::wdf` substrate + `DiodePair` root）、PRD 021 /
ADR 029（`AsymDiode` + 回授拓撲）、白皮書 §6（WDF 白箱電路模擬）、
`docs/tone_revolution/overview.md`（本 Phase 是整個計畫的地基與第一步）
新增 ADR：**無**——本 Phase 不改架構，是同一個 root 介面下的數值/效能升級。
（root 求解「以近似式為生產路徑、以 Newton 為永久 oracle」的政策若延續到
op-amp 家族，由 Phase 03 的 WDF adaptor framework ADR 一併形式化。）

## 1. 背景與決策

`screamer`（PRD 020）與 `sd1`（PRD 021）證明了 WDF 白箱可行，但也證明了它**貴**：
每個過取樣 sample、每聲道解一次 `f(v) = v + R·i(v) − a = 0`，用 warm-start
damped Newton（f64、每輪一個 `f64::exp`）。實測 ~68–72 µs/block，約 memoryless
`ts9` 的 6 倍。Tone Revolution 的目標是「**所有** drive 都能是白箱」，而 chain
允許多個 drive slot——兩顆 WDF 同開就吃掉 >10% deadline。這個成本是整個計畫的
瓶頸，所以它是 Phase 01。

**做法**：Werner et al.（DAFx-16）把二極體 root 方程重排成 **Wright omega**
函數 `ω`（解 `ω + ln ω = x`）的求值；D'Angelo（DAFx-19）再證明 `ω` 本身可用
「多項式起始猜測 + 一次 Newton 修正」逼近，其中修正裡的 `exp`/`log` 也是多項式
＋位元運算近似。整條路徑**零迭代、無 `std::exp`**。

**必須說清楚的定性**：對反並聯二極體對（`i = 2·Is·sinh`），eqn (39) 是
**高精度近似**而非恆等式（它假設「一側導通時另一側電流可忽略」）。所以 Newton
版**不是被刪除的舊碼，而是永久保留的 oracle**（`DiodePair::solve_newton`）：
golden 測試用它量測近似路徑的實際誤差，容差憑量測拍板，不靠論文背書。

### 與參考實作的差異（本專案自行擬合）

參考實作（chowdsp `omega.h` / BYOD）用 D'Angelo 的**三次**起始猜測 + **一次**
修正。實測那組合在我們的 root 上最差 **2.03 mV**（節點電壓）。把修正加到兩次可以
降到 39.5 µV，但踏板成本從 29.8 µs 漲到 40.4 µs。

改為**離線重新擬合起始猜測**——關鍵是**擬合目標取「修正之後」的誤差**，而不是
猜測本身的誤差（均勻的絕對誤差擬合在 `ω ~ 0.01` 的低端相對誤差很差，而 Newton
的二次收斂要求猜測誤差相對於 `ω` 夠小）。得到：

- `x < −4.5`：猜 0（修正後正好得 `e^x`；該區 `ω ≈ e^x`，誤差 ≤ 1.2e-4）
- `−4.5 ≤ x < 8`：**自行擬合的四次多項式**
- `x ≥ 8`：`x − ln x + ln x / x`（只取首項的 `x − ln x` 在 `x = 8` 低了 0.26，
  一次修正救不回來；補第二項後猜測誤差降到 ~1.6e-3，代價一個除法）

**一次**修正即達 30.5 µV / 30.5 µs——**比參考實作兩次修正更準，且更快**。係數、
分區邊界、二項漸近式都是我們自己算的，授權上比移植更乾淨（`log2`/`pow2` 的係數
仍沿用 D'Angelo，MIT）。

`exp_approx` 的 ~6e-4 相對誤差是整條 ladder 的**精度地板**（修正結果被壓在
`ω·6e-4/(1+ω)` 附近），第二、三次修正實測毫無改善——所以「一次」不是省，是收斂點。

## 2. 規格

### `blocks/wdf.rs` → `blocks/wdf/` 目錄

`mod.rs` 保持原路徑與 API（`screamer`/`sd1` 呼叫端零改動），新增
`blocks/wdf/omega.rs`。Phase 03 反正要拆，現在先立目錄省一次搬遷。

### `blocks::wdf::omega`

- `log2_approx` / `pow2_approx`（D'Angelo 係數）、`log_approx` / `exp_approx`
  （IEEE-754 指數欄位做範圍化約，Rust 以 `f32::to_bits`/`from_bits` 取代 C union）。
- `omega_guess`（三區起始猜測，見上）、`omega`（猜測 + `CORRECTIONS` 次修正）。
- 引數以 `max`/`min`（**非** `clamp`，`clamp` 會傳播 NaN）夾在 ±1e30：任何輸入
  ——含 NaN——都回傳有限值（RT 規則 7）。
- 精度全部以 `f64` Newton 參考解量測後 pin 在測試裡，並在 doc comment 標明。

### `DiodePair`

- `solve(a, r)` 簽章不變、回傳 `(v, b)` 不變，內部改走閉式：
  `v = a − Vt·λ·(ω(L + λa/Vt) − ω(L − λa/Vt))`，`λ = sign(a)`（用 `copysign`），
  `L = ln(R·Is/Vt)`。
- `L` 是唯一與 `R` 有關的項，**只在 `r` 改變時**以 `f64` 重算後下轉（`screamer`
  的 `R` 固定，實際只在 `prepare` 算一次；pot 在樹裡的電路才會逐 sample 觸發）。
- 閉式路徑**無狀態**——`reset()` 只清 Newton 的 warm start。
- `solve_newton(a, r)` 為 `pub`：oracle 兼 bench 對照組。

### `AsymDiode`（`sd1`）

**維持 Newton**。非對稱曲線（`m`/`k` 顆數不同）沒有現成的 eqn (39) 形式；仿造的
兩-omega 推廣是我們自創、誤差分析要重做，不值得在地基階段冒險。列為 Phase 04
的選配優化，屆時沿用本 Phase 的 golden 機制驗證。

### 不做

- 不改 WDF 樹結構、不改 `screamer`/`sd1` 的建模範圍（Phase 03/04）。
- 不追 `std::exp` 位元級一致。
- 不引入 cargo feature 或執行期旗標——兩條路徑永遠都在、測試恆跑兩路。

## 3. 驗收標準與實測

### 3.1 `cargo test`（全數綠燈）

| 測試 | 標準 | 實測 |
| ---- | ---- | ---- |
| `omega_is_accurate` | vs f64 Newton 參考，`x ∈ [−30, 2000]` | 最差 **7.0e-4**（bound 1e-3；峰值在 `x ≈ 1400`，是 f32 解析度不是近似誤差） |
| `omega_low_tail_error_is_bounded_by_its_leading_term` | 低尾 `ω ≈ e^x` | 絕對 ≤ 1.5e-4、相對 ≤ 1.2e-2 |
| `omega_is_monotonic` | `x ∈ [−200, 1e7]` 單調、有限 | 通過（含兩個分區接縫） |
| `omega_has_no_step_at_the_region_seams` | 接縫跨越量 vs 真值差 < 2×bound | 通過（接縫不留 kink） |
| `omega_extremes_stay_finite` | ±MAX、±inf、**NaN** | 全部有限且 ≥ 0 |
| `exp_and_log_approximations_are_accurate` | pin 實際精度 | `exp` 相對 6.1e-4、`log` 絕對 3.6e-3 |
| `solve_matches_the_newton_oracle` | `a ∈ [−50, 50]`、4 種 `R` | 最差 **|Δv| = 3.05e-5 V**（bound 5e-5） |
| `solve_matches_the_newton_oracle_spectrally` | 前 10 諧波位準差 | **< 0.1 dB**（實際 ~0.001 dB 量級） |
| `solve_is_stateless` / `solve_tracks_a_changing_port_resistance` | 無狀態、`R` 變動正確 | 逐點 `assert_eq` |
| 既有 `screamer` 全部測試 + drive 家族 character pin | 不得動搖 | 全綠（302 tests） |

> 注意一個必要的測試重整：舊的 `solve_satisfies_the_diode_equation`（殘差
> < 1e-4）改名 `newton_oracle_satisfies_the_diode_equation` 並改測 oracle。
> 原因是**殘差不是閉式路徑該被判的單位**：knee 附近 `R·di/dv ≈ 130`，30 µV 的
> 電壓誤差會放大成 ~5 mV 的殘差。電路輸出的是電壓，所以閉式路徑判電壓
> （`solve_matches_the_newton_oracle`）與頻譜。
> 同理 `asym_diode_matches_diode_pair_when_symmetric` 改對 `solve_newton` 比對
> ——那是兩條**方程式**等價的宣稱，兩邊都該用精確解，容差才能維持 1e-6。

### 3.2 `cargo bench -p lh-dsp`（同一次執行，數字入 `docs/benchmarks.md`）

| Bench | 實測 | 標準 |
| ----- | ---- | ---- |
| `wdf_root_256_solves/omega` vs `/newton` | 2.29 µs vs 29.1 µs = **12.7×** | ≥ 5× ✅ |
| `drive_screamer_4x_oversampled` | **30.5 µs**（2.3% deadline） | ≤ 33 µs ✅ |
| 同機 Newton 版 screamer 對照 | 72.3 µs（5.4%） | — |
| `drive_sd1_4x_oversampled` | 68.0 µs（不變，仍 Newton） | — |

**過程中量到、值得記下來的兩件事：**

1. **root microbench 12.7× 但整顆踏板只有 2.4×，兩者都對。** 閉式 root 是
   **無狀態**的，microbench 的 256 次求解彼此獨立 → CPU 可以 pipeline，量到的是
   **throughput**；在電路裡每次求解都餵給電容狀態、嚴格串列 → 量到的是
   **latency**。把 root 抽掉實測 screamer 剩 **12.4 µs**（≈ `ts9` 的 11.3 µs），
   那是任何 4× 過取樣踏板的地板。**未來估 WDF 成本要用整顆踏板的數字，別用
   microbench 外推。**
2. **分區用真 branch 比 branchless 快 ~7%。** 直覺是「削波器的工作點正好停在
   `x = 8` 接縫上，會一直 mispredict」，所以試了「兩邊都算再 blend」——實測
   screamer 40.4 → 43.3 µs，**變慢**。分支預測器處理得很好，eager evaluation
   純粹是多做工。（結論寫進 `omega_guess` 的 doc comment，避免以後有人再試一次。）

### 3.3 `assert_no_alloc`

`screamer` 選取 + 狂推全程零配置（閉式路徑無 heap、無迭代）。既有 debug 測試涵蓋。

### 3.4 耳朵（**待使用者驗收**）

`screamer` 換閉式 root 後**聽感應與換前一致**——這是等價替換，不是新音色。
30 µV / 0.001 dB 的差距遠低於可聞門檻；真正的音色改動留給 Phase 02（tone stack）
之後。若聽得出差異，請回報，那代表某個假設錯了。

## 4. 產出

- `crates/lh-dsp/src/blocks/wdf/mod.rs`（原 `wdf.rs`，`git mv`）
  ＋ `crates/lh-dsp/src/blocks/wdf/omega.rs`（新）
- `DiodePair::solve` 走閉式、`solve_newton` 為 `pub` oracle；`AsymDiode` 不動
- 測試 26 條（omega ladder + root golden + 頻譜 golden）
- `crates/lh-dsp/benches/effects.rs`：`wdf_root_256_solves` 群組
- `docs/benchmarks.md`：2026-07-27 段
- 原始碼註解保留 D'Angelo (MIT) / Werner / Chowdhury (BSD-3) 出處，並標明
  **哪些係數是我們自己擬合的**
