# Phase 01 — 快速非線性 root：Wright Omega 閉式解

命中目標：#2（讓「所有 drive」的白箱成本可負擔） · 地基
依賴：無（可獨立先做）
關聯 ADR：沿用 ADR 028/029 的 WDF root 介面；本 Phase 不改架構，屬效能/數值升級
來源與授權：`/mnt/chowdsp_wdf/include/chowdsp_wdf/math/omega.h`（**MIT**，D'Angelo）＋
`wdft/wdft_nonlinearities.h`（**BSD-3**）→ 以 Rust 重寫，附出處。

---

## 1. 背景與決策

lion-heart 的 `DiodePair`/`AsymDiode`（`crates/lh-dsp/src/blocks/wdf.rs`）用
**f64 damped Newton 迭代（16 次上限，每次一個 `f64::exp`）**解 WDF 二極體根方程：

```
f(v) = v + R·i(v) − a = 0,   i(v) = 2·Is·sinh(v/nVt)   （對稱）
```

CLAUDE.md 記錄 `screamer`/`sd1` ≈ 68–71 µs/block（約 memoryless `ts9` 的 6 倍，
~5% deadline）——這個成本正是 WDF 目前只能是「深水區奢侈品」、不能當所有 drive
預設削波引擎的原因。「我要所有的 drive」若每顆都背 Newton，CPU 會撐不住多顆同開。

**chowdsp 的做法**：D'Angelo（DAFx-2019）的 **Wright Omega 閉式近似**。WDF 二極體
方程可重排成 Wright omega 函數 `ω`（解 `ω + ln ω = x`）的一次求值——**零迭代**。
且 `ω` 本身用**多項式（Estrin 展開）＋位元運算逼近 log/exp**，全程沒有 `std::exp`/
`std::log` 呼叫、branch 極少。這是 Werner et al. WDF 二極體那條方程的**解析解**，
不是精度打折的捷徑。

**拍板**：在 `blocks::wdf` 新增 Wright omega 求值與 omega 版二極體 root，作為
`DiodePair`/`AsymDiode` 的**新求解路徑**；保留 Newton 版為對照/後備（golden 測試
用）。目標：WDF 削波成本降 ~5–10×，數值更穩健（無迭代收斂邊界）。

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
`2·sinh` 的 omega 化簡。方案（Phase 04 定案，本 Phase 先備介面）：
- (a) 保留 Newton 給非對稱 root（僅 `sd1` 一顆，成本可接受）；或
- (b) 用 D'Angelo 的一般式：非對稱可拆成兩個單向 omega 項的差
  （`i = Is·(exp(v/mVt) − exp(−v/kVt))` → 各支獨立求值再合），與對稱式同構。
  建議 (b)，數學上乾淨、與對稱式共用 omega。

### 2.3 API 相容

- `DiodePair::solve(a, r)`/`AsymDiode::solve(a, r)` 簽章**不變**（回傳 `(v, b)`）；
  內部改走 omega。既有 `screamer`/`sd1` 呼叫端**零改動**。
- 新增 feature/常數旗標或型別參數切換 Newton↔omega，讓 golden 測試能同時跑兩路。

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
- **omega vs Newton golden**：兩路在 `a∈[−50,50]`、多個 `R` 下差 < 1e-3（證明
  omega 是等價替換，不是新音色）。
- **非對稱**（若採 2.2(b)）：`m=k=1` 退化回對稱對；`m≠k` 保留偶次諧波特徵
  （沿用 `wdf.rs` 既有非對稱測試）。
- 多 rate/block、silence→silence。

### 4.2 `cargo bench -p lh-dsp`
- 新增 `wdf_root_omega` 對照 `wdf_root_newton`：**預期 omega ≥ 5× 快**。
- `screamer`/`sd1` 全踏板 bench 重測：**預期從 ~68–71 µs 降至 ~10–15 µs/block**，
  記入 `docs/benchmarks.md` 深水區段（標「Wright Omega 取代 Newton」）。

### 4.3 `assert_no_alloc`
- select `screamer`/`sd1` 並狂推全程無配置（omega 無 heap、無迭代、branch-free）。

### 4.4 耳朵（使用者）
- `screamer`/`sd1` 換 omega 後**聽感應與換前一致**（這是等價替換）；若有差異，
  是 Newton 的容差鬆動或 f32/f64 精度所致，記錄之。真正的音色改動留給後續 Phase。

## 5. 產出清單

- `crates/lh-dsp/src/blocks/wdf.rs`（或新檔 `blocks/wdf/omega.rs`）：omega 階梯 +
  omega 版 root。
- 測試：omega 正確性、殘差、omega↔Newton golden。
- bench：`wdf_root_omega` vs `wdf_root_newton`；更新 `docs/benchmarks.md`。
- 原始碼註解保留 D'Angelo(MIT)/Werner/Chowdhury(BSD) 出處。

## 6. 風險與備註

- **f32 精度**：`Is` 極小（~2.5e-9），`ln(R·Is/Vt)` 在 f32 可能損精度；必要時
  該預算式以 f64 算、熱路徑 omega 以 f32。以 golden 測試把關。
- omega4 在極端 `x` 的單調性/有界性已由 D'Angelo 證明；仍以 ±1e6 狂推測試守 RT 規則 7。
- 這是**整個計畫投報率最高、最獨立的一步**，建議最先做、單獨一個 PR。
</content>
