# Phase 03 — WDF 可組合 adaptor 框架 + R-Type + op-amp 模型

命中目標：#2/#3 的**共同地基**
依賴：Phase 01（omega root）
關聯 ADR：**新開 ADR 031（WDF 可組合框架）**——架構級，continues ADR 028/029
來源與授權：`/mnt/chowdsp_wdf`（**BSD-3**）逐 Rust 重寫；R-Type 散射矩陣由
**R-Solver** 從 netlist 自產（見 Phase 08）。**不搬 BYOD(GPL) 碼。**

---

## 1. 背景與決策

`blocks::wdf` 目前是**手工化約**版：`Capacitor`、`DiodePair`/`AsymDiode`、
`parallel_root`/`parallel_root_with_source`。每個電路（`screamer`/`sd1`）都手推
代數化簡成直線程式碼。這對「一兩顆」很好——最小、無 boxed tree、無配置——但要
支撐「**所有 op-amp overdrive**」與「**自研任意電路**」，需要 chowdsp_wdf 那種
**可組合的 adaptor 樹**：把電路用物件拼出來，而不是每顆手推代數。

`chowdsp_wdf` 的關鍵能力（lion-heart 現在缺）：

- **Series / Parallel adaptor** 帶**阻抗傳遞**（impedance propagation）——子節點
  阻抗變動時，向上重算 port 阻抗；`ScopedDeferImpedancePropagation` 讓多旋鈕同動
  時只重算一次。
- **R-Type adaptor**：處理 series/parallel 化約不掉的拓撲（含 op-amp 回授）。核心
  是 N-port **散射矩陣** `b = S·a`，`S` 由各 port 阻抗算出。
- **op-amp 模型**：以有限增益 `Ag`、輸入阻抗 `Ri`、輸出阻抗 `Ro` **直接建進 R-Type
  散射矩陣**——比 `sd1` 現用的「理想 op-amp 虛短」更接近真實邊界行為。

**證據**（研究 BYOD 全部 WDF drive）：TS/ZenDrive/MXR/RAT 是**同一套範式**，
ZenDrive 的散射矩陣**與 TS 一字不差**。所以有了 R-Type + op-amp，第 3 層一整個
overdrive 家族＝「build 樹 + 貼散射矩陣 + 設二極體」。

**拍板**：graduate `blocks::wdf` 為三件——(1) one-port 原語擴充、(2) Series/Parallel
adaptor 帶阻抗傳遞、(3) N-port R-Type adaptor + op-amp 阻抗式。全程 RT 安全、
`assert_no_alloc` 乾淨。`screamer`/`sd1` 以新框架**重寫並保持位元/聽感等價**
（回歸測試），證明框架正確。

## 2. 規格

### 2.1 One-port 原語擴充（`blocks::wdf`）

照 `chowdsp_wdf/wdft/wdft_one_ports.h`、`wdft_sources.h` 以 Rust 重寫：

- `Resistor { r }`（反射自由：`b = 0`）。
- `Capacitor`（已有，bilinear `R = T/2C`、`b[n] = a[n−1]`）。
- `ResistorCapacitorSeries` / `ResistorCapacitorParallel`（RC 合成 one-port，
  BYOD 大量用；避免多節點）。
- `ResistiveVoltageSource { r, e }`（含內阻電壓源，`b = e`）、
  `CapacitiveVoltageSource`、`ResistiveCapacitiveVoltageSource`（ZenDrive 用）。

每個 one-port 提供 `resistance()`/`conductance()`、`reflected()`、`set_incident(b)`、
`prepare(fs)`、`reset()`。denormal flush 沿用既有慣例。

### 2.2 Series / Parallel adaptor + 阻抗傳遞

Rust 沒有 C++ 那種模板遞迴 tree，選一個 RT 安全的組法（ADR 決定）：

- **選項 (a) const-generic 靜態組合**：`Series<A, B>` / `Parallel<A, B>` 泛型結構，
  編譯期展開，零 dispatch、零配置——最貼近 chowdsp、效能最好，但 Rust 泛型巢狀
  型別會很長（可用 type alias 收斂）。
- **選項 (b) 扁平陣列 + 索引**：把樹壓成固定大小陣列（節點以索引連結），一個
  `WdfTree<const N>` 持有 `[Node; N]`；`prepare` 建拓撲、熱路徑走陣列。較好讀、
  易 codegen（Phase 08），代價是一層間接。
- **選項 (c) 維持手工化約 + 提供可重用 adaptor 函式**（如 `parallel_root` 的推廣：
  `series_reduce`、`parallel_reduce`、阻抗上推 helper），每顆仍手寫直線碼但共用
  數學。最小改動、最貼 lion-heart 現風格，但「可組合」程度最低。

> 建議：**op-amp 家族用 (a) 或 (b)**（因為要靠 R-Type + 自動散射矩陣），tone
> stack/簡單 clipper 可續用 (c)。ADR 031 拍板；傾向 (b) 扁平陣列——對 Phase 08
> codegen 最友善，且 const-generic N 仍可維持零堆積。

阻抗傳遞：子阻抗變動 → 向上重算；提供 `defer_impedance`（等效
`ScopedDeferImpedancePropagation`），多旋鈕同動只重算一次（照
`util/defer_impedance.h`）。

### 2.3 R-Type adaptor + op-amp

照 `chowdsp_wdf/rtype/*.h`（BSD）以 Rust 重寫：

- `RType<const N>`：持有 `[[f32; N]; N]` 散射矩陣 + N 個子 port；`scatter(a) -> b`
  ＝矩陣-向量乘（非 SIMD 版就是 `b[c] = Σ_r S[r][c]·a[r]`，見 `rtype_detail.h`）。
- `set_s_matrix(&[[f32; N]; N])`；`compute()`：`b = S·a` → 推 `b` 給各 port、
  讀回各 port `reflected()` 成新 `a`。
- **root 版**（含非線性）與 **adapted 版**（留一個未適配 port 面對二極體，
  `calc_impedance` 回傳該 port 阻抗 `Ra`，二極體對它解 omega root）——BYOD TS 用
  `RtypeAdaptor<..., 0, ...>`（adapted port index 0）。
- **op-amp**：散射矩陣由 `(Ag, Ri, Ro, 各 port 阻抗)` 算出。BYOD 共用一組
  `Ag=100, Ri≈1e9, Ro≈0.1`。矩陣公式**用 R-Solver 從 netlist 自產**（Phase 08），
  不抄 BYOD。

### 2.4 以新框架重寫 `screamer`/`sd1`（回歸驗證）

- `screamer`（shunt clipper）以新 Parallel adaptor + omega root 重組。
- `sd1`（回授拓撲）：可從「理想 op-amp 虛短」升級為**有限增益 op-amp R-Type**
  （更忠實），或先保持虛短、僅換 omega。ADR 記錄。
- 目標：重寫後對舊版**位元/聽感等價**（golden 回歸），證明框架不改音色。

## 3. 非目標

- **不做 SIMD**（chowdsp 的 xsimd 路徑）——先純量、正確優先；SIMD 是後續優化。
- **不做自動微分/符號求解**——散射矩陣走 R-Solver 離線產生（Phase 08）。
- **不在本 Phase 加新踏板**——本 Phase 只交付框架 + 重寫既有兩顆驗證。新踏板是
  Phase 04+。
- 不移植 BYOD 的 tree/R-Type **程式碼**（GPL）——從 `chowdsp_wdf`(BSD) 重寫。

## 4. 驗收標準

### 4.1 `cargo test`
- **adaptor 阻抗傳遞**：一組 R/C 樹，改子阻抗後 root 阻抗符合手算；`defer` 只
  重算一次（計數驗證）。
- **R-Type 散射**：對一個已知小電路（如 TS 3-port），`b = S·a` 對照離線
  參考解；op-amp 有限增益極限（`Ag→∞`）趨近理想虛短。
- **重寫回歸**：新框架版 `screamer`/`sd1` 對舊版 golden（同輸入輸出差 < 1e-4）。
- **RT 有界**：全樹 ±1e6 狂推、全旋鈕掃，輸出有界、無 NaN。
- 多 rate/block、silence→silence。

### 4.2 `cargo bench`
- `screamer`/`sd1` 新框架版成本對照舊版（**不應顯著變慢**；omega 已在 Phase 01
  降過成本）。R-Type 矩陣乘成本記入 `docs/benchmarks.md`。

### 4.3 `assert_no_alloc`
- 框架全路徑（含 R-Type `compute`）零配置；樹於 `prepare` 建好；N 為 const 上界。

### 4.4 耳朵（使用者）
- 重寫後 `screamer`/`sd1` 與改前**無可聞差異**（純重構驗證）。

## 5. 產出清單

- `crates/lh-dsp/src/blocks/wdf/`（可能拆多檔）：one-ports、adaptors、rtype、
  op-amp、defer_impedance。
- 以新框架重寫 `drive/screamer.rs`、`drive/sd1.rs` + golden 回歸測試。
- **ADR 031**：WDF 可組合框架（組法選項 (a)/(b)/(c) 拍板、R-Type、op-amp 模型、
  與現有手工化約碼的關係）。
- **PRD 023**（正式版，若進主序列）。
- 更新 `docs/benchmarks.md`；原始碼保留 chowdsp(BSD) 出處。

## 6. 風險與備註

- **Rust 泛型 tree 型別爆炸**：選項 (a) 的 `Parallel<Series<..>, ..>` 型別很長；
  用 type alias + `impl` 收斂，或改選項 (b) 扁平陣列。
- **散射矩陣正確性**：這是最容易錯的地方——**務必先把 Phase 08 的 R-Solver 跑
  起來**，用它產生 + 用離線數值電路解對照，別手抄。
- **這是最大、最架構的一步**；建議獨立里程碑、獨立 PR，重寫兩顆既有踏板當「框架
  正確」的活體證明再往下鋪。
</content>
