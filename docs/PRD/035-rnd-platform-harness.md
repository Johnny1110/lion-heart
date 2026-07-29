# PRD 035: 自研平台 —— 驗證 harness、白箱判別套件、食譜、器件擬合工具（Phase 08 第一部分）

狀態：**已實作（2026-07-29）**
日期：2026-07-29
里程碑：Tone Revolution · Phase 08（`docs/tone_revolution/phase/08-rnd-platform.md` §2.2–2.4）
關聯：**ADR 036**（本 Phase 的形狀決策）、ADR 032（矩陣執行期數值構造——本 PRD 一半的
前提）、ADR 033（元件參數政策）、ADR 035（單端口/多端口界線）、PRD 036（`mane`，
用這套工具走完全程的範例踏板）
新增 ADR：**ADR 036**

## 1. 背景與決策

Phase 08 命中目標 **#3：框架要支撐使用者自研踏板**。計畫的五項產出裡，
**§2.1（R-Solver codegen）與 §2.2（SPICE 擬合流程）在落地時已經不成立**，
理由與取代方案在 **ADR 036**，摘要：

- §2.1 要的散射矩陣 codegen **無物可產**——ADR 032 把矩陣改成執行期由 junction
  的 netlist 數值構造，netlist 本身就是 Rust，沒有中間產物。
- §2.2 要的 ngspice 這個環境沒有，而且 Phase 02 早就把 ngspice fixtures 換成
  本專案自己的節點分析 oracle 了；再把外部模擬器變成 CI 相依是開倒車。

拿掉 codegen 之後，「自研平台」實際缺的是**知道自己做對了沒有**。這是本 PRD
的主體。

### 1.1 為什麼特性測試不夠

整個 drive 家族的既有測試是特性測試：有中頻隆起、有偶次諧波、膝部隨頻率移動。
**這些一個錯得很像的電路也全部會通過**。真正需要檢查的那句話——「這棵 WDF 樹
解的是這張電路圖」——在 Phase 08 之前沒有任何測試在檢查。

## 2. 規格

### 2.1 `lh_dsp::testutil::netlist` —— 獨立的節點分析求解器

修正節點分析（MNA），與 `blocks::wdf` **不共用任何程式碼、公式或常數**。

| 項目 | 做法 |
| ---- | ---- |
| 未知數 | 非接地節點電壓 + 每個電壓源/受控源一個支路電流 |
| 電容 | **梯形伴隨模型**：`i = G·v − s`，`G = 2C/T`，`s' = 2Gv − s` |
| 非線性 | 在目前猜測處線性化（電導 `di/dv` + 等效電流源），整個系統重解 |
| 收斂 | 阻尼 Newton——方向是 Newton 的，長度被砍到任一接面每次不超過 5 個熱電壓 |
| 線性解 | 自寫的部分主元高斯消去（不用外部 crate，才能宣稱「不共用」） |
| 元件 | `R` / `C` / `Src` / `Vcvs` / `Pair`（`2Is·sinh`）/ `Asym`（`Is(e^{v/vtf} − e^{−v/vtr})`） |

`Circuit::new` 會**檢查 netlist**：節點編號越界、或沒有恰好一個驅動源，直接 panic
並指出是哪個元件——手寫 netlist 真正會犯的錯是打錯節點號碼，那應該當場失敗而不是
在很後面變成一個錯的數字。

**關鍵性質**：兩邊用同一種離散化，所以是**同一個離散系統的兩種寫法**，
不是同一個連續系統的兩種近似。因此「對到 1e-6」是有意義的門檻而不是運氣。

求解器自己有四條自檢，每條都有封閉式可以對：電阻分壓（精確）、RC 的
−3 dB 轉角、二極體落在自己的 `v–i` 曲線上（電流兩邊相等）、非反相放大器 = 10×。

### 2.2 `lh_dsp::testutil::whitebox` —— 白箱判別套件

| 函式 | 問的問題 | 曲線 | 電路 |
| ---- | -------- | ---- | ---- |
| `memory` | 同一個瞬時輸入會不會給出不同輸出？ | 3.4e-6 | `mane` **1.52**，shunt clipper 0.98 |
| `knee_shift` | 失真量隨頻率變嗎？ | **恰好** 1.000000 | shunt clipper **0.355** |
| `harmonics` → `even_over_odd` / `thd` | 對稱嗎？失真多少？ | — | `mane` 0.43 |
| `bounded` | 灌 ±1e6 V 會發散嗎？ | — | 必須有限 |
| `silent` | 靜音進去是**精確**靜音出來嗎？ | — | 必須是 |
| `static_curve` | 靜態轉移曲線 | — | 對手算或 netlist |

`memory` 是唯一一個能用**單一數字**把白箱與 memoryless 分開的量測，理由與
實作細節（分桶後**先擬二次式再看殘差**，否則曲線的曲率會被算成記憶）在
ADR 036 §2。四個數量級的分離。

兩個模組都放在**函式庫**的 `testutil` 而不是 `tests/`，因為整合測試的模組到不了
`src/`——使用者自研踏板時單元測試寫在 `src/drive/mypedal.rs` 裡，工具必須在那裡
拿得到。

### 2.3 `crates/lh-dsp/tests/whitebox.rs` —— harness 本體

兩個電路，各自**宣告一次、建兩次**（一次 WDF 樹、一次 netlist），比對：

1. **串並聯樹**：shunt RC-二極體削波器（`diode-clipper` 的 Shunt、`screamer` 的輸出級）。
2. **R-type junction**：家族共用的非反相 op-amp 佈局（`ts-wdf` / `zendrive` /
   `king-of-tone` / `diode-clipper` Feedback / `mane`）——**本 Phase 的驗收對象**，
   因為它是框架裡唯一沒有封閉式可核對的部分。

junction 直接用框架自己的公開常數（`NON_INVERTING_NODES` / `NON_INVERTING_PORTS` /
`non_inverting_els`），而參考 netlist 是照那份**文件裡的節點編號**手寫的。
所以改動佈局會弄壞參考——那正是要的耦合。

### 2.4 `docs/tone_revolution/cookbook.md` —— 食譜

驗收標準是「別人照著走得完」。**從第 0 步「這顆該不該用 WDF」開始**
（ADR 035 的單端口/多端口界線），因為那一步錯了後面全白做。

七節：0 選路線 / 1 寫 netlist（沒有 codegen 步驟）/ 2 元件參數從哪來 /
3 拼出踏板（含旋鈕種類、settled-skip、RT 規則）/ 4 三層驗證 / 5 進 registry
（只能追加）/ 6 常見卡點對照表 / 7 `mane` 走完全程的對照。

### 2.5 `tools/fit_device.py` —— 器件級擬合

從 datasheet 的 I–V 點擬 `(Is, n)`，輸出可貼的 Rust 常數。numpy + scipy，**無 SPICE**。
兩個實作決定在 ADR 036 §4：**在 log 電流上**做最小平方（否則 10 mA 的點壓過膝部
所有點），`Is` **在 log 空間**擬合（跨十幾個數量級）。

## 3. 驗收標準與實測

### 3.1 harness 的實測殘差（`cargo test -p lh-dsp --test whitebox`，9 條全綠）

| 比對 | 標準 | 實測 |
| ---- | ---- | ---- |
| 串並聯樹，靜態轉移曲線（15 點，±20 V） | < 2e-6 V | **2.5e-7 V** |
| 串並聯樹，動態雙音+包絡（Newton root） | < 5e-6 V | **1.5e-7 V** |
| 串並聯樹，動態（omega 閉式 root） | < 4e-4 V | **2.1e-4 V** |
| **R-type junction，靜態掃描（11 點，含硬削波）** | `2e-7 + 5e-5·|v|` | **相對 3.2e-5**，掃描全程一致 |
| R-type junction，動態（Newton / omega） | < 3e-4 / 2e-3 V | **2.8e-5 / 1.1e-4 V** |
| 閉式 root 誤差 > 4× Newton root 誤差 | — | **1400×** |
| 兩棵樹：有界（灌 ±1e6 V）、精確靜音 | — | 通過 |

R-type 那個 **3.2e-5 相對誤差**掃描全程一致，這個「常數比例」正是條件數的簽名
而不是矩陣錯的簽名：junction 的元件值跨七個數量級（`Ri` = 1 GΩ 旁邊 `Ro` = 100 Ω）。
−90 dB，聽不到，但它是**任何對照的精度上限**。這個觀察直接產生了 ADR 036 §3
的元件參數政策。

「閉式 root 的誤差是樹的 1400 倍」被單獨釘成一條測試
（`the_closed_form_root_is_the_dominant_error_not_the_tree`）：它讓 harness 對
「自己在量什麼」保持誠實，將來若有人動了 `omega`，這條會先叫。

### 3.2 判別套件的自檢（5 條，在 `testutil::whitebox` 內）

以 `tanh` 當「必須被判為曲線」的對照組、一階低通 + `tanh` 當「必須被判為電路」的
對照組；另外用硬削波正弦的傅立葉級數釘住諧波讀數（基波對封閉式 < 1e-4，
偶次諧波洩漏 < 1e-9）。

### 3.3 全域

`cargo test -p lh-dsp` **461 → 468**（lib）+ harness 9 條；workspace 全綠，
debug 與 release 皆綠（release 24 個測試 binary 全過）。
`cargo fmt --check`、`cargo clippy --workspace --all-targets -- -D warnings` 乾淨。

### 3.4 擬合工具自檢

以 1N4148 的四個 datasheet 點（0.5 V/0.11 mA … 0.7 V/12 mA）擬出
`Is = 8.98e-10, n = 1.645`，殘差 0.023 decades RMS，1 mA 膝部 0.592 V
——與本專案既有的 1N4148 值（2.52e-9 / 1.75，膝部 0.60 V）同一個位置。

## 4. 非目標

- **不做 codegen**（ADR 032 已作廢該路線）。
- **不做 SPICE 電路級對照**（§1，harness 在同一個位置做得更好）。
- **不做 tweakable component 設計模式**（計畫 §2.5，選配）。理由在 ADR 036：
  `diode-clipper` 已經是這個想法的踏板版本，通用版需要 GUI 管線，價值與成本不符。
- **不重跑 ZenDrive 的 MOSFET 擬合**（計畫 §4 的驗收項）——ADR 034 已經處理完，
  重跑不會產生新資訊。

## 5. 已知取捨

- **`testutil` 進了函式庫**，所以 `netlist`/`whitebox` 會被編進 `lh-dsp`。它們是
  `Vec`-based、`f64`、會配置記憶體的離線程式碼，**永遠不可以從音訊路徑呼叫**。
  兩個檔頭都寫了，但這是靠紀律維持的界線。
- **harness 驗證的是框架與拓撲，不是某個 `drive/*.rs` 抄對了 schematic**。
  電路在測試裡宣告，同一組常數同時建兩邊。後者仍然是各踏板自己的手解 AC 分析
  的工作（食譜 §4.2）。這個界線寫在 `tests/whitebox.rs` 檔頭。
- **3.2e-5 是 R-type 對照的精度地板**。比這更嚴的容差是在測 `f32` 不是測電路。

## 6. 產出

- `crates/lh-dsp/src/testutil/netlist.rs`（新）、`crates/lh-dsp/src/testutil/whitebox.rs`（新）
- `crates/lh-dsp/src/testutil.rs`：兩個 `pub mod` + 檔頭說明
- `crates/lh-dsp/tests/whitebox.rs`（新，9 條）
- `docs/tone_revolution/cookbook.md`（新）
- `tools/fit_device.py`（新）
- `docs/adr/036-rnd-platform.md`
