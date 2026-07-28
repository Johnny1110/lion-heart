# Phase 03 — WDF 可組合 adaptor 框架 + R-Type + op-amp 模型

> **已實作（2026-07-28）— PRD 025 / ADR 032。** 計畫與實際交付的差異記在這裡，
> 計畫本身不改：
>
> 1. **§2.5 的硬前置與後備對調：散射矩陣改為執行期由 netlist 數值構造，不做
>    R-Solver 符號 codegen。** 連帶 **`tools/wdf_codegen/` 未產出**。決定性理由正是
>    §6 最擔心的那件事——「別手抄」：數值構造**沒有可抄的東西**，矩陣在執行期由拓撲
>    導出，不可能與它宣稱模擬的拓撲脫節；符號 codegen 反而多出「產生器 → 貼進
>    Rust → 有人改了元件值」這條會腐化的縫。次要理由是 R-Solver 不在本機（其 CAS
>    後端可裝，但接起來等於自己重寫一次），以及成本實測可接受：4-port 重建 114 ns、
>    含 op-amp 的 6×6 458 ns，且只在 block 邊界 + settled-skip。**這也直接服務目標
>    #3**——使用者加一顆自研電路只要寫 netlist，不需要 Python 或 codegen 步驟。
>    驗證管道（`SᵀR⁻¹S = R⁻¹`、對照閉式 adaptor、端對端對照樹、op-amp 理想極限）
>    完整交付，日後真要補符號 codegen，驗收標準已經現成。
> 2. **§2.3 的 up port 有位置限制**（計畫未提，實作時踩到）。adapted 的定義就是
>    `R_up = R_戴維南`，所以 up port 不能放在被回授壓到近零阻抗的節點——第一版驗收
>    測試把它掛在 op-amp 輸出腳，閉迴路輸出阻抗 ~1e-12 Ω，量到的增益剛好差兩倍。
>    修法是放到串聯輸出電阻之後（真實踏板本來就長這樣）。
> 3. **§2.1 少交付 `ResistiveCapacitiveVoltageSource`**（ZenDrive 才用得到，延到
>    Phase 04）。框架已能以 `Series<Resistor, CapacitiveVoltageSource>` 表達，複合
>    型別只是快捷路徑；而 chowdsp 那顆的遞推我無法從其實作反推出對應電路，不憑猜測
>    交付。root 版 R-Type 同樣未做（計畫本就說「後續有電路需要再補」）。
> 4. **§4.2「不應顯著變慢」——實際變快。** 同一輪隔離量測：框架版並聯節點 2.28 µs
>    vs 改寫前的手工化約 helper 2.92 µs，**快 22 %**（adaptor 把比值與阻抗快取住，
>    舊 helper 每個 sample 做兩次除法）。
> 5. **§2.4 的等價重寫實測 ~1e-8**（要求 1e-4），對**改寫前的 HEAD** 端對端渲染
>    對照。另外發現：理想電流源沒有 one-port 形式，但在 root 上不需要——注入 `I`
>    恰是入射波平移 `R·I`，這是 `sd1` 能框架化而不改音色的關鍵等式。
>
> 完整數字與測試清單見 PRD 025 §3 與 `docs/benchmarks.md`。


命中目標：#2/#3 的**共同地基**
依賴：Phase 01（omega root）；**前置工作項：R-Solver 最小 bootstrap（§2.5，
原 Phase 08 提前）**——沒有乾淨的矩陣來源就沒有可驗收的 R-Type
關聯 ADR：**新開 ADR（暫定 031，WDF 可組合框架）**——架構級，continues ADR 028/029
來源與授權：`/mnt/chowdsp_wdf`（**BSD-3**）逐 Rust 重寫；R-Type 散射矩陣由
**R-Solver** 從 netlist 自產（見 §2.5 / Phase 08）。**不搬 BYOD(GPL) 碼。**

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
adaptor 帶阻抗重算、(3) N-port R-Type adaptor + op-amp 阻抗式；外加 (0) R-Solver
最小 bootstrap（§2.5，硬前置）。全程 RT 安全、`assert_no_alloc` 乾淨。
`screamer`/`sd1` 以新框架**重寫並保持數值/聽感等價**（回歸測試），證明框架正確。

## 2. 規格

### 2.1 One-port 原語擴充（`blocks::wdf`）

照 `chowdsp_wdf/wdft/wdft_one_ports.h`、`wdft_sources.h` 以 Rust 重寫：

- `Resistor { r }`（反射自由：`b = 0`）。
- `Capacitor`（已有，bilinear `R = T/2C`、`b[n] = a[n−1]`）。
- `ResistorCapacitorSeries` / `ResistorCapacitorParallel`（RC 合成 one-port，
  BYOD 大量用；避免多節點）。
- `ResistiveVoltageSource { r, e }`（含內阻電壓源，`b = e`）、
  `CapacitiveVoltageSource`、`ResistiveCapacitiveVoltageSource`（ZenDrive 用）。
  註：**bias 電源就是 `e = 4.5V` 的 ResistiveVoltageSource**（MXR/ZenDrive 的
  Vb 半軌偏壓）——不需要額外機制，但踏板輸出端記得 DC block（家族既有慣例）。
- `PolarityInverter`（King of Tone 的 clipper 樹用到；一行波變換，補齊）。

每個 one-port 提供 `resistance()`/`conductance()`、`reflected()`、`set_incident(b)`、
`prepare(fs)`、`reset()`。denormal flush 沿用既有慣例。

### 2.2 Series / Parallel adaptor + 阻抗重算

Rust 沒有 C++ 那種「子↔父互持參考」的模板樹（chowdsp 的 `connectToParent` 在
borrow checker 下是反模式）。三個組法（ADR 拍板）：

- **選項 (a) 泛型靜態組合＋擁有式子樹（v2 建議）**：`Series<A, B>` /
  `Parallel<A, B>` 泛型結構**擁有**子節點（`struct Parallel<A: OnePort, B:
  OnePort> { a: A, b: B, .. }`），編譯期單態化成直線碼——零 dispatch、零配置、
  與 chowdsp 樹一一對應（`WDFParallelT<CapacitiveVoltageSource, Resistor>` ↔
  `Parallel<CapacitiveVoltageSource, Resistor>`），移植 BYOD 拓撲時逐行可對照。
  型別名很長 → per-pedal `type` alias 收斂。波交換由**根往下驅動**（root 對
  tree 呼叫 `reflected()` / `set_incident(b)`，adaptor 遞迴到子節點）——正是
  現行 `screamer`/`sd1` 手寫直線碼的形狀，只是變成可組合。
- **選項 (b) 扁平陣列 + 索引**：`WdfTree<const N>` 持 `[Node; N]`。**v2 降為
  備選**：異質節點得包 enum + 逐 sample match dispatch，R-Type 的 const-generic
  維度塞不進統一 `Node`；「對 codegen 友善」不成立——codegen 產出 (a) 的泛型樹
  原始碼一樣容易（Phase 08 產的是 Rust 文字，不是執行期結構）。
- **選項 (c) 維持手工化約 + 可重用 helper**：`parallel_root` 一族的推廣。保留——
  簡單電路（shunt clipper 等）不必強遷框架。

**阻抗傳遞（v2 重新設計，比 chowdsp 簡單）**：不做 chowdsp 的「子變動向上通知
＋ `ScopedDeferImpedancePropagation` 抑制重複」——那套存在是因為它的樹有 parent
指標。我們的樹是擁有式、由根驅動：**`calc_impedance()` 從根一次遞迴（後序：先子
後己）重算全樹**，在 block 邊界、且僅當有旋鈕動時呼叫（settled-skip 慣例）。
多旋鈕同動天然只算一次，**不需要 defer 機制**；熱迴圈內零阻抗運算。

### 2.3 R-Type adaptor + op-amp

照 `chowdsp_wdf/rtype/*.h`（BSD）以 Rust 重寫：

- `RType<const N, Ports>`：持有 `[[f32; N]; N]` 散射矩陣 + 子 port tuple；
  `compute()`：收集各 port `reflected()` 成 `a` → `b = S·a`（矩陣-向量乘，
  `b[c] = Σ_r S[r][c]·a[r]`，見 `rtype_detail.h`）→ 推 `b[k]` 給各 port。
  維度註記：**up port 也佔一列/行**——TS 的 R-node 是「3 個子 port + 1 個
  up port」＝ **4×4** 矩陣（初稿說「TS 3-port」不準確）。
- **adapted 版**（主用）：up port 反射自由，面向樹的其餘部分/二極體 root；
  `calc_impedance` 以閉式回傳 up-port 阻抗 `Ra`，二極體對它解 root——BYOD TS 用
  `RtypeAdaptor<..., 0, ...>`（up-port 佔 S 的 index 0）。**root 版**（R-node
  本身當根、無 up port）後續有電路需要再補。
- **op-amp**：散射矩陣元素是 `(Ag, Ri, Ro, 各 port 阻抗)` 的**閉式有理式**——
  op-amp 的受控源在 R-Solver 推導時就融進矩陣，執行期只是代數求值（無矩陣
  求逆）。BYOD 共用一組 `Ag=100, Ri≈1e9, Ro≈0.1`。矩陣公式**用 R-Solver 從
  netlist 自產**（§2.5），不抄 BYOD。
- **S 重算時機（RT 關鍵）**：pot 在 R-Type 的某個 port 內（Zen 的 voice、MXR 的
  dist）→ 動旋鈕要重算整組 S（一票有理式求值，純算術、無配置）——**只在 block
  邊界、settled-skip**，不逐 sample（BYOD 平滑期間逐 sample 重算，我們不跟）。
  pot 在 R-Type 之外（TS 的 drive 在 P3 的 R6‖C4 一-port）→ 只重算該 one-port
  與二極體的 `logR_Is_overVt`，便宜。

### 2.4 以新框架重寫 `screamer`/`sd1`（回歸驗證）

- `screamer`（shunt clipper）以新 Parallel adaptor + omega root 重組。
- `sd1` **保持理想虛短不動**（v2 明確化）：本 Phase 是**嚴格等價重構**，不做任何
  音色改動——有限增益 op-amp 的「忠實版 TS/SD-1」屬 Phase 04（新 key 追加）。
  Phase 邊界乾淨：03 = 框架正確性證明；04 = 新聲音。
- 目標：重寫後對舊版**數值等價**（golden 回歸，容差 1e-4；浮點運算順序會變，
  不承諾位元相等），證明框架不改音色。

### 2.5 R-Solver 最小 bootstrap（自 Phase 08 提前）

- 確認 R-Solver 的相依（CAS 後端）與授權；跑通「netlist → 散射矩陣符號式」。
- 用 TS netlist 產出矩陣，**驗證方式＝數值**：對隨機一組 port 阻抗，把 R-node
  接的 junction 以離線 MNA/電路方程數值解出參考散射行為，對照符號式求值（此法
  同時服務 §4.1 驗收與 Phase 08 harness；可另與 BYOD 已發布矩陣做人工抽查
  ——閱讀 GPL 文本做驗證不是複製，但**產物一律以自產為準**）。
- 產出：`tools/wdf_codegen/` 的雛形（跑 R-Solver + 把符號式轉成 Rust `fn
  s_matrix(...) -> [[f32; N]; N]`）。打磨成「使用者級」工具鏈留在 Phase 08。
- **後備**：若 R-Solver 授權/相依有問題——R-node 散射矩陣有已發表的數值構造法
  （Werner et al.「grand unified theory of WDF」系的 MNA 式推導），可在
  `prepare`/knob-rate 數值算 S（N≤8 的小矩陣求逆，仍 RT-safe）；成本是每顆
  踏板多一份 junction 描述。ADR 記錄取捨。

## 3. 非目標

- **不做 SIMD**（chowdsp 的 xsimd 路徑）——先純量、正確優先；SIMD 是後續優化。
- **不做執行期符號求解**——散射矩陣走 R-Solver 離線產生（§2.5 bootstrap、
  Phase 08 打磨）；數值 S 構造只作後備。
- **不在本 Phase 加新踏板、不改任何既有踏板的聲音**——本 Phase 只交付框架 +
  等價重寫既有兩顆驗證。新踏板與忠實版升級是 Phase 04+。
- 不移植 BYOD 的 tree/R-Type **程式碼**（GPL）——從 `chowdsp_wdf`(BSD) 重寫。

## 4. 驗收標準

### 4.1 `cargo test`
- **adaptor 阻抗重算**：一組 R/C 樹，改子元件值後根阻抗符合手算；重算只在
  block 邊界觸發、settled 時跳過（計數驗證）。
- **R-Type 散射**：TS 的 4×4 R-node，隨機多組 port 阻抗下 `b = S·a` 對照離線
  數值參考解（§2.5 的驗證管道）；op-amp 有限增益極限（`Ag→∞`、`Ri→∞`、
  `Ro→0`）趨近理想虛短（與 `sd1` 現行化約互證）。
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

- `crates/lh-dsp/src/blocks/wdf/`（拆多檔）：one-ports、adaptors（Series/
  Parallel/PolarityInverter）、rtype（含 op-amp 閉式阻抗）。
  （v2：**無 defer_impedance**——根驅動全樹重算取代之，見 §2.2。）
- §2.5：`tools/wdf_codegen/` 雛形 + TS netlist + 數值驗證管道。
- 以新框架重寫 `drive/screamer.rs`、`drive/sd1.rs` + golden 回歸測試。
- **ADR**（暫定 031）：WDF 可組合框架（組法 (a)/(b)/(c) 拍板與 Rust 所有權
  設計、R-Type、op-amp 模型、S 重算時機、與現有手工化約碼的關係、R-Solver
  vs 數值 S 後備）。
- **PRD**：落地時於主序列取號。
- 更新 `docs/benchmarks.md`；原始碼保留 chowdsp(BSD) 出處。

## 6. 風險與備註

- **Rust 泛型 tree 型別爆炸**：選項 (a) 的 `Parallel<Series<..>, ..>` 型別很長；
  per-pedal type alias 收斂。真炸到不可維護（編譯時間/錯誤訊息），再退 (c)
  手工化約——(b) 不是逃生口（見 §2.2）。
- **散射矩陣正確性**：這是最容易錯的地方——§2.5 的「R-Solver 產生 + 隨機阻抗
  數值對照」是本 Phase 的硬前置，別手抄、別跳過。
- **借用結構**：R-Type 的 up-port 阻抗 `Ra` 依賴各子 port 阻抗——注意先
  `calc_impedance()` 全樹、再算 `Ra`、再餵二極體 `logR_Is_overVt` 的順序；
  寫成一條「knob 變動 → 全鏈重算」的單一入口避免亂序。
- **這是最大、最架構的一步**；建議獨立里程碑、獨立 PR，重寫兩顆既有踏板當「框架
  正確」的活體證明再往下鋪。
</content>
