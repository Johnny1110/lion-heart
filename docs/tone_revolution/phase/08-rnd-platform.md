# Phase 08 — 自研平台工具鏈：從電路到白箱踏板

命中目標：**#3（框架支撐我未來自研音色踏板）** · 計畫收官
依賴：Phase 03（WDF 框架穩定）；與 Phase 04 交織（第一批踏板就用這條工具鏈產生
散射矩陣，避免手抄）
關聯 ADR：沿用 ADR 031；工具鏈本身在 repo 外圍（`tools/`、`sim/`、`docs/`）
來源與授權：**R-Solver**（`github.com/jatinchowdhury18/R-Solver`）；SPICE
（ngspice/LTSpice）；擬合用 Python（numpy/scipy）。皆與 lion-heart 授權相容或為
外部工具（不進 runtime）。

---

## 1. 背景與決策

前面 Phase 交付「框架 + 一批名踏板」；本 Phase 交付**讓使用者自己造踏板**的能力
——這是目標 #3 的本體，也是把「Tone Revolution」從「移植」升級成「平台」的關鍵。

BYOD 的高效率來自兩套外圍工具（lion-heart 現在沒有）：

1. **R-Solver**：從電路 **netlist** 自動產生 R-Type **散射矩陣**——使用者不必手推
   那個嚇人的 N×N 符號矩陣。這也是 §4「授權合規」要求的：**自產矩陣、不抄 BYOD**。
2. **SPICE → 擬合 流程**（`BYOD/sim/`）：每顆踏板都有 LTSpice `.asc` 原理圖 →
   暫態模擬 → Python 擬合腳本，把二極體/電晶體參數擬到實測曲線（ZenDrive 的
   `Is=5.24e-10` 就是這樣來的）。

**拍板**：把這兩條工具鏈在 lion-heart repo 立起來 + 一個**驗證 harness** + 一份
**「新增一顆 WDF 踏板」食譜**，讓自研有路可循、有據可驗。

## 2. 規格

### 2.1 netlist → 散射矩陣 codegen（`tools/wdf_codegen/`）

- **netlist 格式**：定義一個簡單的電路描述（節點、R/C、電位器、op-amp、二極體、
  非線性 root port）。可直接用 R-Solver 的 netlist 格式（`tools/netlists/*.txt`）。
- **跑 R-Solver**：腳本呼叫 R-Solver 產生散射矩陣（符號式，以 port 阻抗為變數）。
- **codegen 成 Rust**：把散射矩陣輸出成 Phase 03 框架吃的 Rust
  （`fn s_matrix(impedances) -> [[f32; N]; N]`）——貼進踏板檔即可。這對應 Phase 03
  「組法 (b) 扁平陣列」最友善（codegen 產索引 + 矩陣）。
- **產物入庫**：每顆踏板的 netlist 進 repo（`tools/netlists/`），散射矩陣是
  **可重生的產物**（授權乾淨：netlist 是事實、矩陣是自產數學）。

### 2.2 SPICE → 參數擬合流程（`sim/`，仿 BYOD）

- **原理圖**：每顆踏板一個 LTSpice/ngspice netlist（`.asc`/`.cir`），可跑 DC 掃描
  與暫態。
- **擬合腳本**（`sim/<pedal>/fit.py`）：對非線性元件（二極體/電晶體/MOSFET）擬合
  `Is/n/Vt/β`，把 SPICE 曲線對到 WDF root，最小化殘差 → 得「聽起來對」的參數。
- **用途**：(a) 移植名踏板時擬合其實際元件；(b) **使用者自研時**，畫個電路、跑
  SPICE、擬合、丟進框架——不必憑空猜參數。

### 2.3 驗證 harness（`crates/lh-dsp/tests/` 或離線 bin）

- **golden vs 高精度數值電路解**：一個離線「參考電路解」（極小步長 / 高倍 OS 的
  同電路數值積分），當 WDF 踏板的 golden；新踏板加一條「靜態轉移曲線 + 動態
  頻率相依門檻」對照。
- **（選）golden vs SPICE**：把 SPICE 暫態輸出當 golden，對照 WDF 輸出（容差內；
  不追位元對拍）。
- **白箱判別測試模板**：頻率相依削波、對稱/非對稱、飽和有界——做成可複用的測試
  helper，新踏板套用即可。

### 2.4 「新增一顆 WDF 踏板」食譜（`docs/tone_revolution/cookbook.md`）

一份 step-by-step：
1. 畫電路 / 找 schematic → 寫 netlist（`tools/netlists/mypedal.txt`）。
2. `tools/wdf_codegen` 跑 R-Solver → 得散射矩陣 Rust。
3. （選）SPICE 擬合二極體/電晶體參數。
4. 在 `drive/mypedal.rs` 用 Phase 03 框架拼樹 + 貼矩陣 + 設 root。
5. 套白箱判別測試模板 + character pin + bench。
6. registry 追加（append-only）、livery、plugin id、`clap-validator`。
- 附一個**完整可跑的範例**（用 2.6 的自研範例踏板走完全程）。

### 2.5 （選）Tweakable component 層

BYOD 每顆把每個 R/C 都暴露成可調（`CircuitQuantity` + schematic SVG），對**設計/
調音**極有用。lion-heart 可做輕量版：一個 debug/design 模式，讓使用者即時掃某顆
踏板的元件值聽差異（不必進正式 faceplate）。純開發輔助，不影響 runtime/preset。

### 2.6 自研範例踏板（證明平台可用）

用整條工具鏈**設計一顆全新的、非移植的**踏板（例如使用者想要的某個特定音色——
可與使用者討論規格），走完食譜全程，當作平台的 end-to-end 驗證與教學範例。

## 3. 非目標

- **工具鏈不進 runtime/plugin**——R-Solver/SPICE/擬合都是**離線開發工具**；audio
  path 只吃 codegen 出來的靜態矩陣。
- **不做 GUI 電路編輯器**——netlist 文字檔即可（GUI 是遠期）。
- **不追全自動 netlist→踏板**——半自動（codegen 矩陣，人拼樹 + 校準）即可。
- **不散布 SPICE 模型庫的專有內容**——用公開/自建 model。

## 4. 驗收標準

- **codegen**：對一個已知電路（如 TS 3-port），`wdf_codegen` 產的散射矩陣對照
  離線參考解一致；產出的 Rust 能編譯並通過該踏板測試。
- **SPICE 擬合**：對 ZenDrive（已知 BYOD 擬合值）重跑擬合，得到相近參數（驗證
  流程正確）。
- **驗證 harness**：至少一顆 Phase 04 踏板用 golden-vs-數值解通過。
- **食譜**：**使用者（或另一位）能照食譜，從 netlist 到綠燈踏板走完**——這是目標
  #3 的實質驗收。
- **範例踏板**（2.6）：一顆非移植的自研踏板進 registry、全綠、耳朵驗收。

## 5. 產出清單

- `tools/wdf_codegen/`（R-Solver 封裝 + Rust codegen）、`tools/netlists/`。
- `sim/<pedal>/`（LTSpice/ngspice netlist + `fit.py`），仿 BYOD `sim/` 結構。
- 驗證 harness（離線參考電路解 + 白箱判別測試模板）。
- `docs/tone_revolution/cookbook.md`（新增 WDF 踏板食譜 + 完整範例）。
- （選）tweakable-component design 模式。
- 一顆自研範例踏板。

## 6. 風險與備註

- **R-Solver 相依**：確認其授權與可用性；若不便直接用，可自寫一個小型符號散射
  矩陣產生器（成本較高但可控）。**這條要在 Phase 03/04 動手前就跑通**——否則 04
  的散射矩陣會沒有乾淨來源。
- **SPICE 工具鏈**：ngspice（開源）優於 LTSpice（授權/平台）作為 CI 可跑的擬合後端。
- **這是「平台化」的收官**，但其中 §2.1 codegen 其實應**提早到 Phase 03/04 之間**
  就 bootstrap（哪怕最小版），讓名踏板的矩陣一開始就走乾淨來源、不手抄 GPL。
</content>
