# Phase 08 — 自研平台工具鏈：從電路到白箱踏板

> **落地修正框（2026-07-29）。** 本 Phase 已實作，但本文是在 **Phase 03 之前**
> 寫的，而 Phase 03 換掉了它的地基。與本文出入甚大，以
> **ADR 036 + PRD 035/036** 為準：
>
> 1. **§2.1 的 codegen 整節作廢——無物可產。** 本文要求建 `tools/wdf_codegen/`
>    跑 **R-Solver** 產生符號散射矩陣、netlist 存 `tools/netlists/`。**ADR 032**
>    把「符號 codegen（硬前置）」與「執行期數值構造（後備）」對調了：矩陣現在由
>    junction 自己的 netlist 在 `calc_impedance()` 裡數值解出來，而那個 netlist
>    就是 Rust（`JEl` / `Junction`）。沒有中間產物，也就沒有 codegen 步驟。
>    **`tools/wdf_codegen/` 與 `tools/netlists/` 都不存在，也不會存在。**
>    （本文 §6 自己把這列為風險並指定「後備＝數值 S 構造」——後備成了主線。）
> 2. **§2.2 的 SPICE 流程改成器件級擬合。** 本文要 LTSpice/ngspice 原理圖 + 暫態
>    模擬 + `sim/<pedal>/fit.py`。這個環境沒有 ngspice，而 Phase 02 早已把它的
>    ngspice fixtures 換成本專案自己的節點分析 oracle。實作改為
>    **`tools/fit_device.py`**（numpy/scipy，無模擬器）：擬一條指數 I–V 本來就
>    不需要模擬器。電路級的對照由第 3 點做得更好。**`sim/` 不存在。**
> 3. **§2.3 的 harness 是本 Phase 真正的主體**，而且比本文設想的更進一步：
>    不是「高倍過取樣的數值積分當 golden」，而是
>    **`lh_dsp::testutil::netlist`——一個獨立的修正節點分析求解器**，與
>    `blocks::wdf` 不共用任何程式碼、公式或常數。兩邊用**同一種梯形離散化**，
>    所以是同一個離散系統的兩種寫法，殘差因此可歸因（PRD 035 §3.1）。
>    驗收對象是 **R-type junction**——框架裡唯一沒有封閉式可核對的部分。
> 4. **§2.5 的 tweakable component 模式沒做**（本文標為選配）。`diode-clipper`
>    （PRD 030）已經是這個想法的踏板版本；通用版需要 GUI 管線，價值與成本不符。
>    食譜改為說明「怎麼把一個元件值變成一顆旋鈕」（`Ctl::Trim`）。
> 5. **§4 的「對 ZenDrive 的 MOSFET 重跑擬合」不做。** **ADR 034** 已在 Phase 04
>    處理完，而且更正了本文的判讀依據——BYOD 的 Zen 參數**不是**被它的 P1/P3
>    接線 bug 污染（那是離線對獨立 LTspice 削波器擬的），重擬的真正理由是
>    「擬合 `Is·sinh`、求值 `2·Is·sinh`」。重跑不會產生新資訊。
> 6. **§2.6 的範例踏板是 `mane`**（PRD 036），規格由使用者定：**不只削波，還要
>    音色調教**。Focus 掃增益腳的電容（**迴路內**，決定哪些頻率先破），
>    Bass/Mid/Treble 走 Phase 02 的被動 JCM800 網路（**迴路後**）。
>    **它沒有新增任何 junction、adaptor 或 root**——那正是平台可用的證明。
> 7. **關聯 ADR 不是「沿用 ADR 031」**（本文標頭）。本 Phase 的 ADR 是 **036**；
>    031 是 ADAA。

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

> v2 註：**最小雛形已提前到 Phase 03 §2.5**（跑通 R-Solver + TS 矩陣 + 數值
> 驗證管道）——本節是把雛形打磨成「使用者級」工具。

- **netlist 格式**：直接用 R-Solver 的 netlist 格式（`tools/netlists/*.txt`），
  不自創格式。
- **跑 R-Solver**：腳本呼叫 R-Solver 產生散射矩陣（符號式，以 port 阻抗為變數）。
  先確認其 CAS 後端相依與授權（Phase 03 §2.5 已驗）。
- **codegen 成 Rust**：把散射矩陣輸出成 Phase 03 框架吃的 Rust——
  `fn s_matrix(...) -> [[f32; N]; N]` + 一段**泛型樹組裝骨架**（對應 Phase 03
  組法 (a)；codegen 產的是 Rust 原始碼文字，泛型樹與陣列一樣好產）——人再補
  faceplate/校準。
- **驗證內建**：codegen 順手產「隨機阻抗數值對照」測試（Phase 03 §2.5 的管道
  模板化），每顆新矩陣自帶正確性測試。
- **產物入庫**：每顆踏板的 netlist 進 repo（`tools/netlists/`），散射矩陣是
  **可重生的產物**（授權乾淨：netlist 是事實、矩陣是自產數學）。
- **後備**（若 R-Solver 長期不可用）：Werner 系數值 S 構造（Phase 03 §2.5），
  代價是 runtime 多一份 junction 描述 + 小矩陣求逆。

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

- **codegen**：對一個已知電路（TS 的 4×4 R-node），`wdf_codegen` 產的散射矩陣
  對照隨機阻抗數值參考解一致；產出的 Rust 能編譯並通過該踏板測試。
- **SPICE 擬合**：對 ZenDrive 的 MOSFET-diode 重跑擬合——**驗收對象是「擬合後
  的 WDF 靜態/暫態曲線貼合 SPICE 真值」**，不是「重現 BYOD 的參數數字」
  （v2：BYOD 的 Zen 參數繞著其實作 bug 擬合，見 Phase 04 §2.2——複現它的數字
  反而是錯的）。
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

- **R-Solver 相依**：確認其 CAS 後端相依與授權（v2：已定為 **Phase 03 §2.5 的
  硬前置**，不會拖到本 Phase 才發現問題）；後備＝數值 S 構造（§2.1）。
- **SPICE 工具鏈**：ngspice（開源、可 script）優於 LTSpice（授權/平台）作為
  CI 可跑的擬合後端；Phase 02 的 tone stack AC fixtures 已先用上 ngspice，
  管線是現成的。
- **這是「平台化」的收官**：codegen 雛形在 Phase 03、tone stack fixtures 在
  Phase 02 先行——本 Phase 的增量是擬合流程、食譜、tweakable 層與自研範例踏板。
</content>
