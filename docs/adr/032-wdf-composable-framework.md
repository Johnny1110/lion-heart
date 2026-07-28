# ADR 032: WDF 可組合框架 — 擁有式泛型樹 + 執行期數值散射矩陣

狀態：**已採納（2026-07-28）**
關聯：`docs/tone_revolution/phase/03-wdf-adaptor-framework.md`（來源計畫）、
PRD 025（落地規格）、ADR 028（WDF Tube Screamer）、ADR 029（WDF 回授 overdrive）、
PRD 022（omega root）
影響範圍：`blocks::wdf`（拆成五檔）、`drive::screamer`、`drive::sd1`
**是 Phase 04（op-amp overdrive 家族）的地基**

## Context

`blocks::wdf` 到 PRD 021 為止是**手工化約**版：`Capacitor`、`DiodePair`/
`AsymDiode`、`parallel_root`/`parallel_root_with_source`，每個電路各自手推成一段
直線程式碼。兩顆踏板剛好，但 Tone Revolution 目標 #2（「我全部的 drive 都要」）
與 #3（自研踏板平台）要的是**把電路拼出來**，不是每顆手推代數。

缺的三件（`chowdsp_wdf` 有、本專案沒有）：可組合的 Series/Parallel adaptor 帶阻抗
重算、處理 series/parallel 化約不掉之拓撲的 **R-Type**（N-port 散射矩陣）、以及
建進散射矩陣的 **op-amp 有限增益模型**。

## Decision

### 1. 組法：泛型靜態組合 + 擁有式子樹（計畫選項 (a)）

`Parallel<A, B>` / `Series<A, B>` / `PolarityInverter<A>` **擁有**子節點，電路因此
是一個型別：

```rust
type ClipTree = Parallel<ResistiveVoltageSource, Capacitor>;   // screamer
type FeedbackTree = Parallel<Resistor, Capacitor>;             // sd1
```

單態化成直線碼——零 dispatch、零配置、與 chowdsp 樹一一對應（移植 BYOD 拓撲時
逐行可對照）。型別名長 → per-pedal `type` alias，如上。

選項 (b)（扁平陣列 + 索引）**不採用**，理由同計畫 §2.2。選項 (c)（手工化約
helper）**保留**：`parallel_root` 一族仍是公開 API，供整個拓撲就是一個並聯節點的
電路使用。但它不再是「另一份實作」——
`parallel_adaptor_matches_the_hand_reduced_helper` 把兩者釘在一起，快捷路徑不可能
悄悄與框架分岔。

波交換**由根往下驅動**（`reflected()` 上行、`incident(b)` 下行），正是
`screamer`/`sd1` 手寫碼原本的形狀。

### 2. 阻抗傳遞：根驅動全樹後序遞迴

不做 chowdsp 的「子變動向上通知 + `ScopedDeferImpedancePropagation` 抑制重複」。
那套存在是因為它的樹有 parent 指標；Rust 的擁有式樹裡 parent 指標是反模式，而且
**不需要**：`calc_impedance()` 從根一次後序遞迴（先子後己）重算全樹。多旋鈕同動
天然只算一次，不需要 defer 機制。呼叫時機是 block 邊界、且僅當旋鈕真的動過
（settled-skip，沿用 `eq::chain` / `eq::tonestack` 慣例）；**熱迴圈內零阻抗運算**。

### 3. 散射矩陣：**執行期數值構造**，不走 R-Solver 符號 codegen

**這是對計畫最大的偏離。** 計畫 §2.5 把「R-Solver 最小 bootstrap」列為本 Phase 的
硬前置，數值構造列為後備。本 ADR 把兩者對調。

做法直接從「port 是什麼」推出來。`a_k = v_k + R_k·i_k` 說的是：外界在 port `k` 呈現
一個 EMF 為 `a_k`、內阻為 `R_k` 的戴維南源；而 `b_k = 2·v_k − a_k` 把答案讀回來。
所以——把每個 port 照這個樣子 stamp 進 junction 的 MNA、解出各 port 電壓，反射波
就有了；**對每個基底向量 `a = e_k` 各解一次，就填出 `S` 的第 `k` 行**。這與
`eq::tonestack`（ADR 030）從 netlist 抽狀態空間用的是同一個技巧。

adapted up port 的參考阻抗＝junction 在該 port 的戴維南阻抗，用工程師在板子上會做的
方式量：灌 1 A、讀電壓。設成這個值正是 `S[0][0] = 0` 的來源。

**為什麼對調：**

- **可驗證性，也就是計畫自己最擔心的那件事。** 計畫把「散射矩陣正確性」列為本
  Phase 最大風險，指示「別手抄、別跳過」。數值構造**沒有可抄的東西**——矩陣在執行期
  由拓撲導出，不可能與它宣稱模擬的那個拓撲脫節。符號 codegen 反而多出「產生器 →
  貼進 Rust → 有人手動改了元件值」這條會腐化的縫。
- **環境事實。** R-Solver 不在本機；其 CAS 後端（sympy）可從 PyPI 裝，但把它接起來
  等於自己重寫一次 R-Solver，而那個工具的產物我還是得驗證。
- **成本可接受，且量過。** 4-port 重建 **114 ns**、含 op-amp 受控源的 6×6 系統
  **458 ns**；只在 block 邊界、且只在旋鈕動過時。逐 sample 路徑仍只是一次 N×N
  矩陣-向量乘。
- **直接服務目標 #3。** 使用者加一顆自研電路只要寫一份 junction netlist，**不需要
  Python、不需要 codegen 步驟、不需要重新編譯產生器**。

**沒有產出 `tools/wdf_codegen/`**（計畫 §5 列的產物之一），因為執行期路徑讓它失去
理由。誠實界定：若日後某個 junction 大到 rebuild 進不了預算，符號 codegen 隨時可加，
而且**驗收標準已經現成**——下面那組測試就是它的對照管道。

#### `S` 怎麼證明是對的（不靠推導本身背書）

| 測試 | 它獨立在哪裡 |
|---|---|
| `wire_junctions_conserve_power` | 純導線 junction 無耗散 ⟹ `SᵀR⁻¹S = R⁻¹`。**純代數，與構造完全不共用推理**——符號錯、stamp 轉置、節點編號錯都無法僥倖通過 |
| `rtype_reproduces_the_parallel_adaptor` / `..._series_adaptor` | 對照獨立推導（且與 chowdsp 逐項比對過）的三-port 閉式，逐格比 |
| `rtype_tracks_the_equivalent_adaptor_tree` / `rtype_tracks_a_series_tree` | 端對端：同一電路兩條完全不同的算術路徑，跑 1000–2000 sample 含電抗狀態 |
| `op_amp_converges_on_the_ideal_virtual_short` | 對照教科書 `1 + Rf/Rg`，並要求逼近**單調**於開迴路增益 |
| `a_lossy_junction_absorbs_power` / `topology_actually_matters` | 反向對照：內部電阻真的進了矩陣；不同拓撲真的給不同矩陣 |

### 4. op-amp = 三個 junction 元件

`op_amp(in_p, in_n, out, internal, ag, ri, ro)` 展開成 `Res(Ri)` +
`Vcvs(gain = Ag)` + `Res(Ro)`。受控源在**建矩陣時**就折進 `S`，執行期是純算術、
無矩陣求逆——與計畫 §2.3 要的性質相同，只是折進去的時機從「離線符號推導」變成
「block 邊界的數值解」。

### 5. adapted up port 的位置限制（實測發現）

adapted 的定義就是 `R_up = R_戴維南`，所以 **up port 不能放在被回授壓到近零阻抗的
節點**（op-amp 的輸出腳就是）。第一版驗收測試正是踩到這點：把 up port 直接掛在
op-amp 輸出，閉迴路輸出阻抗約 1e-12 Ω，被退化保護夾住後 `S[0][0] ≠ 0`，量到的增益
剛好差兩倍。修法是把 up port 放到串聯輸出電阻之後——那也是真實踏板的樣子，而經典
op-amp overdrive 的做法則是讓 up port 面向二極體網路。這條規則寫進了模組文件。

### 6. S 重算時機

pot 在某個 port 內 → 動旋鈕重算整組 `S`，**只在 block 邊界、settled-skip**。
BYOD 在平滑期間逐 sample 重算，本專案不跟：那是為「人在轉旋鈕」付的、不必要的代價。

### 7. 理想電流源在 root 上不需要 adaptor

`sd1` 的理想 op-amp 把一個電流強灌進回授節點。**理想電流源沒有 WDF one-port 形式**
（port 阻抗無限大；`ResistiveCurrentSource` 是有限內阻的諾頓形式）——但在 root 上
不需要：注入 `I` 恰好等於把入射波平移 `R·I`，`R` 不變。框架版因此寫成
`diode.solve(a + r*i_g, r)`，再把 `2v − a`（用**未平移**的 `a`）交還給樹。這條等式
是 `sd1` 能框架化而不改音色的關鍵。

## Consequences

**好的**

- **等價重寫，實測 ~1e-8。** `screamer`/`sd1` 改寫到框架上，對**改寫前的 HEAD** 端
  對端渲染對照：screamer rms `1.246873566e-1` vs `1.246873615e-1`、sd1
  `1.490910999e-1` vs `1.490911029e-1`——相對誤差 ~1e-8，比計畫要求的 1e-4 緊四個
  數量級，純粹是浮點結合律重排。兩顆的 golden 也以**原樣保留的舊實作**釘在測試裡
  （`the_framework_rewrite_is_numerically_equivalent`）。
- **框架版比手工化約還快。** 同一輪隔離量測（256 sample = 一個 64-frame block 的 4×
  過取樣量）：`wdf_parallel_tree` **2.28 µs** vs 舊 helper `wdf_parallel_helper`
  **2.92 µs`，快 ~22 %。adaptor 把比值與阻抗都快取住，舊 helper 每個 sample 做兩次
  除法。計畫 §4.2 只要求「不應顯著變慢」。
- 加電路 = 拼型別（series/parallel）或寫一份 netlist（R-Type），引擎不動。
- `blocks::wdf` 從 2 檔（665 + 422 行）拆成 5 檔，測試 +28 條。

**要付的**

- **旋鈕動一次要解 N+1 個小線性系統**，而不是套一串有理式。已量測（114 ns / 458 ns），
  且 settled 時完全跳過。這是換掉符號 codegen 的直接代價。
- **junction 內部元件值是 `&'static`**，動態值要掛在 port 上。這對 pot 不是限制——
  pot 掛 port 本來就是 WDF 慣例，而 port 阻抗變動已經走 `calc_impedance`。真需要可變
  內部元件時再擴充。
- **固定上限**：`MAX_PORTS = 8`、`MAX_J_NODES = 12`、`MAX_J_VCVS = 4`（MNA ≤ 15×15，
  ~2 KB stack）。超過會被 `debug_assert` 擋下。
- **泛型型別名很長**。per-pedal `type` alias 收斂；真炸到不可維護再退選項 (c)。
- 巢狀 series adaptor **會翻極性**（`Series<A, Series<B,C>>` 不是三元素串聯鏈），
  要靠 `PolarityInverter` 補回。移植拓撲時這是最容易錯的一步，所以扁平 R-Type
  junction 被當成對照物釘住它（`rtype_tracks_a_series_tree`）。

**沒做的**

- **`tools/wdf_codegen/`**（計畫 §5）——理由見上，執行期路徑取代之。
- **root 版 R-Type**（R-node 自己當根、無 up port）——計畫 §2.3 說「後續有電路需要再
  補」，目前沒有。
- **`ResistiveCapacitiveVoltageSource`** 延到 Phase 04（ZenDrive 才用得到）。框架已能
  以 `Series<Resistor, CapacitiveVoltageSource>` 表達，複合型別只是快捷路徑；而
  chowdsp 那顆的遞推我無法從其實作反推出對應電路，不憑猜測交付。
- **SIMD**（chowdsp 的 xsimd 路徑）——計畫 §3 明列非目標。
- **不加新踏板、不改任何既有踏板的聲音**——計畫 §3 明列。有限增益 op-amp 的「忠實版」
  是 Phase 04 的新 key。
- 沒有移植 BYOD 的 tree/R-Type 程式碼（GPL-3），也沒有抄任何已發表的散射矩陣。
  結構與符號慣例參考 `chowdsp_wdf`（BSD-3）以 Rust 重寫，散射關係在本專案自行推導
  後才與其實作逐項比對。
