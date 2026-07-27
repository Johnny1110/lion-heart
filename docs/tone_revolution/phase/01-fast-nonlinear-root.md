# Phase 01 — 快速非線性 root：Wright Omega 閉式解

命中目標：#2（讓「所有 drive」的白箱成本可負擔） · 地基
依賴：無（可獨立先做）
關聯 ADR：沿用 ADR 028/029 的 WDF root 介面；本 Phase 不改架構，屬效能/數值升級
來源與授權：`/mnt/chowdsp_wdf/include/chowdsp_wdf/math/omega.h`（**MIT**，D'Angelo）＋
`wdft/wdft_nonlinearities.h`（**BSD-3**）→ 以 Rust 重寫，附出處。

> ## ✅ 已實作 — **PRD 022**（2026-07-27），待使用者耳朵驗收
>
> 全部驗收標準達成，**且效能與精度都超過計畫預期**。實作與本文的落差：
>
> 1. **起始猜測是我們自己擬合的四次多項式，不是移植 D'Angelo 的三次**（§2.1
>    原本寫「omega1..omega4，係數照 `omega.h`」）。關鍵是**擬合目標取「一次
>    Newton 修正之後」的誤差**。結果：一次修正就達 **30.5 µV**——比參考實作
>    「三次猜測 + 兩次修正」的 39.5 µV **更準**，而且更快（30.5 vs 40.4 µs）。
>    分區邊界（−4.5 / 8）與二項漸近式 `x − ln x + ln x/x` 同樣是自己算的。
>    授權上比移植更乾淨；只有 `log2`/`pow2` 的係數沿用 D'Angelo（MIT）。
>    函式因此命名 `omega_guess`（不是 `omega3`）——它已不是「第三階近似」。
> 2. **`omega4` 的精度定性錯誤已修正。** §4.1 原本寫「相對誤差 < 1e-4」——實測
>    參考 ladder 的 `omega4` 最差**相對 3.7e-2、絕對 4.5e-2**（峰值在 `x ≈ 7`
>    的分區接縫）。同樣地 `exp_approx` 是 6.1e-4 相對、`log_approx` 是 3.6e-3
>    絕對，都與「~1e-5」的印象差兩個數量級。**`exp_approx` 的誤差就是整條
>    ladder 的精度地板**（第二、三次修正實測毫無改善）——這也是「一次修正」
>    不是省而是收斂點的原因。
> 3. **效能實測 = root microbench 12.7×，整顆踏板 2.4×，兩個數字都對。**
>    閉式 root 無狀態 → microbench 的 256 次求解可 pipeline，量到 throughput；
>    電路裡每次求解餵給電容狀態、嚴格串列，量到 latency。抽掉 root 實測
>    screamer 剩 **12.4 µs**（≈ `ts9`），那是 4× 過取樣的地板。**§4.2 的
>    「≤ 33 µs」達成（30.5 µs），但未來估 WDF 成本要用整顆踏板，別拿
>    microbench 外推。**
> 4. **分區用真 branch 比 branchless 快 ~7%**（40.4 → 43.3 µs 變慢）。§6 沒預期
>    到這點——直覺以為工作點停在接縫會一直 mispredict，實測分支預測器處理得很好。
> 5. §2.2 的 **(a) 保留 Newton 給 `AsymDiode`** 照計畫執行，`sd1` 維持 68 µs。
>
> 詳細數字、測試表與 bench 見 **`docs/PRD/022-wright-omega-root.md`** 與
> `docs/benchmarks.md`（2026-07-27 段）。以下原文保留為當時的規劃依據。

---

## 1. 背景與決策

lion-heart 的 `DiodePair`/`AsymDiode`（`crates/lh-dsp/src/blocks/wdf.rs`）用
**f64 damped Newton 迭代（16 次上限，每次一個 `f64::exp`）**解 WDF 二極體根方程：

```
f(v) = v + R·i(v) − a = 0,   i(v) = 2·Is·sinh(v/nVt)   （對稱）
```

`docs/benchmarks.md` 記錄 `screamer`/`sd1` ≈ 68–71 µs/block（約 memoryless `ts9`
的 6 倍，~5% deadline）——這個成本正是 WDF 目前只能是「深水區奢侈品」、不能當所有
drive 預設削波引擎的原因。chain 允許多個 drive slot（`drive`、`drive2`…疊踏板是
常規玩法），「我要所有的 drive」若每顆都背 Newton，兩顆 WDF 同開就吃掉 >10%
deadline。

**chowdsp 的做法**：D'Angelo（DAFx-2019）的 **Wright Omega 閉式近似**。WDF 二極體
方程可重排成 Wright omega 函數 `ω`（解 `ω + ln ω = x`）的求值——**零迭代**。
且 `ω` 本身用**多項式（Estrin 展開）＋位元運算逼近 log/exp**，全程沒有 `std::exp`/
`std::log` 呼叫、branch 極少。

**數學定性（v2 修正）**：閉式解的精確範圍要說清楚——對**單向單二極體**
（`i = Is·e^{v/Vt}` 型），omega 是該方程的**精確解析解**（Werner et al.,
DAFx-16 "generalized diode clipper" eqn 10）。對**反並聯對（sinh）**，同論文的
eqn (18)「Good」/ eqn (39)「Best」是**高精度近似**（利用「一側導通時另一側電流
可忽略」，交越區誤差最大）——非逐位元等價。所以 Newton 版**不是可拋棄的舊碼**，
是永久的 oracle：golden 測試以它界定 omega 路徑的實際誤差，容差憑量測拍板。

**拍板**：在 `blocks::wdf` 新增 Wright omega 求值與 omega 版對稱二極體 root，作為
`DiodePair` 的新求解路徑；`AsymDiode` **先維持 Newton**（見 §2.2）。保留 Newton 版
為 oracle/後備。目標：root 求解本身 ≥5× 快；全踏板 2–3×（成本地板見 §4.2），
且無迭代收斂邊界。

## 2. 規格

### 2.1 `blocks::wdf::omega` 模組（新）

移植 `omega.h` 的近似階梯，以 Rust `f32`（熱路徑）+ 選配 `f64`：

- `log2_approx` / `log_approx` / `pow2_approx` / `exp_approx`：位元 union +
  Estrin 多項式（係數照 `omega.h`，MIT）。Rust 用 `f32::from_bits`/`to_bits`
  取代 C 的 `union`。
- `omega1`..`omega4`：一至四階 Wright omega 近似。`omega4(x) = y − (y − exp(x−y))
  /(y+1)`，`y = omega3(x)`（在 omega3 上做一次修正）。
- 對外只需 `omega4`（品質檔位），另暴露 `omega3`（更省，供未來調度）。

> 註：BYOD 後期改用外部 `math_approx::wright_omega<3,3>`（本沙盒未 checkout）；
> in-tree `omega.h` 已足夠且授權明確（MIT），以它為準。

### 2.2 omega 版二極體 root

對稱對（`DiodePair`）反射波，照 `wdft_nonlinearities.h` eqn(39)「Best」式：

```
λ  = sign(a)
la = λ·a/Vt
b  = a − 2·Vt·λ·( omega(logR_Is_overVt + la) − omega(logR_Is_overVt − la) )
```

其中 `Vt = n·Vt_single`、`R_Is = R·Is`、`logR_Is_overVt = ln(R·Is/Vt)`（`R` 變動
時重算，`prepare`/knob 移動時）。輸出節點電壓 `v = (a + b)/2`（供 `shape()` 取用）。

**非對稱**（`AsymDiode`，SD-1）：兩個方向二極體數不同（m/k），無法用單一
`2·sinh` 的 omega 化簡。兩案：
- (a) **保留 Newton 給非對稱 root**（僅 `sd1` 一顆在用，~71 µs 已實測可接受）。
- (b) 仿 eqn (39) 的構造做**非對稱兩-omega 近似**（每支各以自己的 `m·nVt`/`k·nVt`
  求 omega 再合成）。注意：這**不是**現成公式——對稱式 (39) 的誤差分析靠
  對稱性，非對稱版是我們自己的推廣，交越區誤差必須以 Newton oracle 重新驗證。

**拍板（v2 翻轉）**：本 Phase 走 **(a)**——風險零、範圍小、不擋任何後續。
(b) 列為 Phase 04 的選配優化：等 op-amp 家族鋪開、若多顆非對稱踏板同開讓
profile 說話了再做，並沿用本 Phase 的 golden 機制驗證。

### 2.3 API 相容

- `DiodePair::solve(a, r)` 簽章**不變**（回傳 `(v, b)`），內部改走 omega；
  Newton 版保留為 `solve_newton`（`pub(crate)`，oracle/測試用）。`AsymDiode`
  不動。既有 `screamer`/`sd1` 呼叫端**零改動**。
- **不引入 cargo feature 或執行期旗標**——兩條路徑永遠都在、測試恆跑兩路，
  避免組態矩陣。
- 檔案組織：`blocks/wdf.rs` 就此拆成 `blocks/wdf/` 目錄（`mod.rs` 原樣
  re-export，路徑不變）＋ `blocks/wdf/omega.rs`——Phase 03 反正要拆，現在先立
  目錄省一次搬遷。

## 3. 非目標

- 不改 WDF 樹結構、不改 `screamer`/`sd1` 的電路建模範圍（那是 Phase 03/04）。
- 不追 `std::exp` 位元級一致——omega 近似在容差內即可（見驗收 §4.1）。
- 不移植 `math_approx` 整包（未 checkout、且非必要）——只取 omega 階梯。

## 4. 驗收標準

### 4.1 `cargo test`
- **omega 正確性**：`omega4(x)` 對照高精度參考（牛頓解 `ω+lnω=x`）在 `x∈[−20,20]`
  相對誤差 < 1e-4；大 `x` 趨近 `x−ln x`、極負 `x` 趨近 0。
- **二極體方程殘差**：omega 解代回 `a = v + R·i(v)`，全輸入（含 ±1e6 狂推）殘差
  在容差內、有界、無 NaN/inf（RT 規則 7）。
- **對稱性**：`v(−a) = −v(a)`（對稱對）。
- **omega vs Newton golden**：兩路在 `a∈[−50,50]`、多個 `R` 下逐點量測
  `|v_omega − v_newton|`——**上界先估 1e-3 V、實測後 pin 更緊的值**（誤差來源
  = eqn(39) 交越區近似 + omega4 近似，都有界）。加一條**頻譜級**判定：同一
  正弦掃描下，前 10 個諧波位準差 < 0.1 dB（證明是等價替換，不是新音色——
  聽感等價由頻譜差保證，比單點電壓差更貼近本意）。
- 多 rate/block、silence→silence。

### 4.2 `cargo bench -p lh-dsp`
- 新增 `wdf_root_omega` 對照 `wdf_root_newton`（純 root microbench）：
  **驗收 = omega ≥ 5× 快**。
- `screamer` 全踏板 bench 重測（v2 修正——初稿的 10–15 µs 不可達）：全踏板成本
  地板是 4× oversampler + 線性級（`ts9` 同機制 ≈ 11.4 µs），**預期 ~68–71 →
  ~20–30 µs（2–3×）**；**驗收 = ≤ 33 µs（2.5% deadline）**，實測值記入
  `docs/benchmarks.md` 深水區段（標「Wright Omega 取代 Newton」）。`sd1`
  維持 Newton（§2.2 拍板），數字不變、註記即可。

### 4.3 `assert_no_alloc`
- select `screamer` 並狂推全程無配置（omega 無 heap、無迭代、branch-free）。

### 4.4 耳朵（使用者）
- `screamer` 換 omega 後**聽感應與換前一致**（這是等價替換）；若有差異，
  是 Newton 的容差鬆動或 f32/f64 精度所致，記錄之。真正的音色改動留給後續 Phase。

## 5. 產出清單

- `blocks/wdf.rs` → `blocks/wdf/` 目錄化（`mod.rs` re-export 保路徑）＋
  `blocks/wdf/omega.rs`：omega 階梯 + omega 版對稱 root（Newton 保留為
  `solve_newton`）。
- 測試：omega 正確性、殘差、omega↔Newton golden（電壓差 + 諧波頻譜差）。
- bench：`wdf_root_omega` vs `wdf_root_newton`；更新 `docs/benchmarks.md`。
- 原始碼註解保留 D'Angelo(MIT)/Werner/Chowdhury(BSD) 出處。
- PRD：落地時於主序列取號。

## 6. 風險與備註

- **f32 精度**：`Is` 極小（~2.5e-9），`ln(R·Is/Vt)` 在 f32 可能損精度；此類
  「R 變動時才重算」的預算式以 f64 算好再下轉，熱路徑 omega 以 f32。以 golden
  測試把關。
- **極端輸入**：±1e6 狂推時 omega 引數 `|x| ~ 1e7`——f32 可表示，但要確認近似
  階梯在此範圍單調、有界（omega(x) → x − ln x 漸近）；必要時對引數夾限（夾限點
  遠在音訊範圍外，不影響音色）。狂推測試守 RT 規則 7。
- omega4 在音訊範圍的精度已由 D'Angelo 論文界定；我們的 golden 對 Newton oracle
  再驗一次，不必信任移植過程。
- 這是**整個計畫投報率最高、最獨立的一步**，建議最先做、單獨一個 PR。
</content>
