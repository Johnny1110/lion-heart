# PRD 025: WDF 可組合 adaptor 框架 + R-Type + op-amp

狀態：**已實作（2026-07-28）— 待使用者耳朵驗收**
日期：2026-07-28
里程碑：Tone Revolution · Phase 03（`docs/tone_revolution/phase/03-wdf-adaptor-framework.md`）
關聯：PRD 020 / ADR 028（WDF Tube Screamer）、PRD 021 / ADR 029（WDF 回授 overdrive）、
PRD 022（omega root）
新增 ADR：**032（WDF 可組合框架）** — 組法拍板、散射矩陣來源、op-amp、up port 限制都在那裡

## 1. 背景與決策

`blocks::wdf` 原本是手工化約版：每個電路手推成一段直線碼。這對兩顆踏板剛好，但
「所有 op-amp overdrive」與「自研任意電路」需要**可組合的 adaptor 樹**。本 PRD 交付
框架本身，並以**等價重寫既有兩顆踏板**當作框架正確的活體證明——**本 Phase 不加新
踏板、不改任何既有踏板的聲音**。

**與 Phase 03 計畫的偏離**（詳見 ADR 032）：計畫 §2.5 把「R-Solver 符號 codegen」列為
硬前置、數值構造列為後備；本次**對調**——散射矩陣改為從 junction netlist 在執行期
數值構造。核心理由是計畫自己最在意的那件事：**沒有可抄的東西就抄不錯**。連帶
`tools/wdf_codegen/` 未產出。

## 2. 規格

### 2.1 `blocks::wdf` 拆檔與 `Wdf` trait

`mod.rs`（hub + trait + 保留的化約 helper）、`one_port.rs`、`adaptor.rs`、
`rtype.rs`、`diode.rs`、`omega.rs`。

`Wdf` 是每個元件與 adaptor 對上層呈現的介面：`calc_impedance` /
`resistance` / `conductance` / `reflected` / `incident` / `prepare` / `reset`。

### 2.2 One-port 原語

`Resistor`、`Capacitor`、`ResistorCapacitorSeries`、`ResistorCapacitorParallel`、
`ResistiveVoltageSource`（bias 電源就是 `e = 4.5V` 的它）、`ResistiveCurrentSource`、
`CapacitiveVoltageSource`。

**複合型別是快捷路徑，不是新物理**：`ResistorCapacitorSeries` 就是
`Series<Resistor, Capacitor>`，`rc_composites_match_the_generic_trees` 逐 sample 釘住
這件事。沒列的形狀仍可用樹表達（R–C–source 腳就是
`Series<Resistor, CapacitiveVoltageSource>`）。

### 2.3 Series / Parallel / PolarityInverter + 阻抗重算

擁有式泛型樹（ADR 032 選項 (a)），per-pedal `type` alias 收斂型別名。
**阻抗傳遞**：`calc_impedance()` 從根一次後序遞迴重算全樹，在 block 邊界、
settled-skip；**無 defer 機制、無 parent 指標**。熱迴圈內零阻抗運算。

### 2.4 R-Type adaptor + op-amp

`RType<N, M, P>` 持 `[[f32; N]; N]` 散射矩陣與 `M = N−1` 個子 port 的 tuple
（`PortSet` trait，macro 展開 1..=7 元）。**port 0 是 up port**，adapted。

`S` 由 `Junction`（`nodes` / `els` / `ports`）在 `calc_impedance` 時數值構造：把每個
port stamp 成「EMF `a_k` 串 `R_k`」，對每個基底向量 `a = e_k` 解一次 MNA，
`S[j][k] = 2·v_j − δ_jk`。上限 `MAX_PORTS=8`、`MAX_J_NODES=12`、`MAX_J_VCVS=4`
（MNA ≤ 15×15、~2 KB stack、零配置）。

**op-amp** = `op_amp(...)` 展開的三個 junction 元件（`Ri`、`Vcvs(Ag)`、`Ro`）。受控源
在建矩陣時折進 `S`，執行期純算術。

**up port 位置限制**（實測發現，ADR 032 §5）：adapted 即 `R_up = R_戴維南`，所以 up
port 不可放在被回授壓到近零阻抗的節點（op-amp 輸出腳）。

### 2.5 以新框架重寫 `screamer` / `sd1`

- `screamer` → `Parallel<ResistiveVoltageSource, Capacitor>` + omega root。
- `sd1` → `Parallel<Resistor, Capacitor>` + `AsymDiode` root，**理想虛短不動**。
  理想電流源沒有 one-port 形式，但在 root 上不需要：注入 `I` 恰是入射波平移 `R·I`。

## 3. 驗收標準與實測

### 3.1 `cargo test`（lh-dsp 337 → 365 條，全綠；workspace 全綠）

**散射矩陣正確性**（計畫列為本 Phase 最大風險）：

| 測試 | 標準 | 實測 |
| ---- | ---- | ---- |
| `wire_junctions_conserve_power` | 導線 junction ⟹ `SᵀR⁻¹S = R⁻¹`（純代數，不共用構造推理） | 通過，2 拓撲 × 3 組阻抗 |
| `rtype_reproduces_the_parallel_adaptor` / `..._series_adaptor` | 對照獨立推導的三-port 閉式，逐格 | 全數 < 1e-4 |
| `rtype_tracks_the_equivalent_adaptor_tree` / `rtype_tracks_a_series_tree` | 同電路兩條算術路徑，含電抗狀態 | 2000 / 1000 sample 內 < 2e-4 |
| `the_up_port_is_reflection_free` | `S[0][0] = 0` | < 1e-5 |
| `a_lossy_junction_absorbs_power` / `topology_actually_matters` | 反向對照 | 通過 |

**op-amp**：`op_amp_converges_on_the_ideal_virtual_short` — `Ag→∞`/`Ri→∞`/`Ro→0` 下
對照 `1 + Rf/Rg`，三組 Rf/Rg 全數 < 0.1 %，且逼近**單調**於開迴路增益；
`finite_open_loop_gain_falls_short` 釘住 `Ag=100` 時真的達不到理想增益（有限增益模型
存在的理由）。

**框架**：`impedance_recomputes_through_the_whole_tree`（含「重算前必須是舊值」的
settled-skip 契約）、`prepare_reaches_the_leaves`、`adapted_ports_are_reflection_free`、
`adaptors_conserve_power`、`polarity_inverter_is_an_involution`、
`rc_composites_match_the_generic_trees`、`the_matrix_follows_a_moving_pot_on_demand`、
`bounded_when_slammed`（±1e6）、`silence_stays_silent`。

**等價重寫**：`the_framework_rewrite_is_numerically_equivalent`（兩顆各 200 000 sample、
掃振幅／掃頻／sd1 另掃 drive pot），對**原樣保留在測試裡的舊實作**，worst |Δ| < 1e-4。

**對真正的改寫前程式碼**（HEAD worktree 端對端渲染，掃三個旋鈕、400 blocks）：

| 踏板 | 改寫前 rms | 改寫後 rms | 相對誤差 |
|---|---|---|---|
| `screamer` | 1.246873615e-1 | 1.246873566e-1 | ~4e-8 |
| `sd1` | 1.490911029e-1 | 1.490910999e-1 | ~2e-8 |

比計畫要求的 1e-4 緊四個數量級；差異是浮點結合律重排。

### 3.2 `cargo bench -p lh-dsp`

**同一輪隔離量測**（256 sample = 一個 64-frame block 的 4× 過取樣量）：

| Bench | 中位數 | 讀法 |
| ----- | ------ | ---- |
| `wdf_parallel_tree` | ~2.28 µs | 框架版並聯節點 |
| `wdf_parallel_helper` | ~2.92 µs | 改寫前的手工化約 helper，**慢 22 %** |
| `wdf_rtype4_scatter` | ~3.42 µs | 4-port R-Type 逐 sample 矩陣-向量乘 |
| `wdf_rtype4_rebuild` | ~114 ns | 旋鈕動一次的 4×4 重建 |
| `wdf_rtype4_opamp_rebuild` | ~458 ns | 含受控源的 6×6 重建 |

框架比它取代的手工化約碼**更快**——adaptor 把比值與阻抗快取住，舊 helper 每個 sample
做兩次除法。計畫 §4.2 只要求「不應顯著變慢」。

整顆踏板：`drive_screamer` ~31.8 µs、`drive_sd1` ~68.8 µs，對 PRD 022 那輪的
30.5 / 68.0 µs 分別 +4.3 % / +1.2 %；同輪的機器校準基準
（`wdf_root_256_solves` omega 2.29→2.33、newton 29.1→27.1）顯示本容器跨場次自然
波動約 ±7 %，**兩顆都落在噪聲內**。

### 3.3 `assert_no_alloc`

框架全路徑零配置：樹於建構時組好、`S` 用固定大小 stack 陣列（MNA ≤ 15×15）、
`RType::reflected`/`incident` 只動 `[f32; M]` stack 陣列。

### 3.4 耳朵（**待使用者驗收**）

`screamer` / `sd1` 改寫前後應**無可聞差異**——這是純重構。測試說相對誤差 1e-8，
耳朵是最後一關。

## 4. 非目標

- 不加新踏板、不改既有踏板聲音（有限增益 op-amp 的忠實版屬 Phase 04 新 key）。
- 不做 SIMD；不做執行期符號求解。
- 不移植 BYOD 的 tree/R-Type 程式碼（GPL-3），不抄任何已發表的散射矩陣。

## 5. 已知取捨

- **旋鈕動一次解 N+1 個小線性系統**（vs 符號式求值）。已量測、settled 時跳過。
- **junction 內部元件值是 `&'static`**；動態值掛 port（pot 掛 port 本是 WDF 慣例）。
- **巢狀 series adaptor 會翻極性**，要 `PolarityInverter` 補回——移植拓撲最易錯的一步，
  已用扁平 R-Type junction 當對照物釘住。
- **固定上限** `MAX_PORTS=8` / `MAX_J_NODES=12` / `MAX_J_VCVS=4`。
- `tools/wdf_codegen/`、root 版 R-Type、`ResistiveCapacitiveVoltageSource` 未交付
  （理由見 ADR 032「沒做的」）。

## 6. 產出

- `crates/lh-dsp/src/blocks/wdf/`：`mod.rs`（trait + hub + 保留 helper）、
  `one_port.rs`、`adaptor.rs`、`rtype.rs`、`diode.rs`（`omega.rs` 未動）
- `crates/lh-dsp/src/drive/screamer.rs`、`sd1.rs`：改寫到框架 + golden 回歸
- `crates/lh-dsp/benches/effects.rs`：`bench_wdf_framework` 群組
- `docs/adr/032-wdf-composable-framework.md`、`docs/benchmarks.md`
