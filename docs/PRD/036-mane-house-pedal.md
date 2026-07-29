# PRD 036: `mane` —— 自研範例踏板：削波與音色調教分開在兩個地方（Phase 08 第二部分）

狀態：**已實作（2026-07-29）— 待使用者耳朵驗收**
日期：2026-07-29
里程碑：Tone Revolution · Phase 08（`docs/tone_revolution/phase/08-rnd-platform.md` §2.6）
關聯：**ADR 036**（自研平台的形狀）、PRD 035（驗證 harness 與食譜——本顆是它的
end-to-end 驗證）、ADR 032（可組合 WDF 框架）、ADR 033（元件參數政策）、
PRD 023 / ADR 030（tone stack 引擎）、PRD 026（`ts-wdf`，共用同一個 junction）
新增 ADR：**無**（ADR 036 已載明決策）

## 1. 背景與決策

計畫 §2.6 要一顆**非移植的自研踏板**，走完整條食譜，當作平台的 end-to-end 驗證。
使用者的規格只有一句話：**「我不僅要削波，還要音色調教。」**

那句話指向一個真實的設計缺口。一顆 drive 踏板的 Tone 旋鈕幾乎永遠在削波器
**之後**，所以它決定你聽到什麼，但**從來不決定什麼東西被破**。那是一個控制做了
半件事，也是為什麼那麼多 overdrive 只有一種個性加上一個亮度設定。

`mane` 把這件事拆成兩個作用在**電路不同位置**的控制：

| 旋鈕 | 作用位置 | 改變什麼 |
| ---- | -------- | -------- |
| **Focus** | 放大器的**增益腳**，在回授迴路**裡面** | **哪些頻率拿到增益**，因此哪些頻率先碰到二極體 |
| **Bass / Mid / Treble** | 級後的被動網路 | 你聽到什麼，帶著真實網路的旋鈕互動與天生音色 |

兩者都不是「靜態曲線 + 濾波器」做得到的。Focus 尤其：動它改變的是**失真的形狀**，
不是頻譜的形狀。

## 2. 規格

### 2.1 Focus，物理上是什麼

非反相級的增益是 `1 + Zf/Zg`，而這裡的增益腳 `Zg` 是 `R_G` 串一顆電容。
低於那條腿的轉角，電容掐住回授電流，增益掉到 unity——乾淨。高於它，全增益。
**Focus 就是那顆電容**，掃兩個十倍頻程：

| Focus | `C_g` | 轉角 | 聽起來 |
| ----- | ----- | ---- | ------ |
| 0 | 470 nF | 72 Hz | 全頻都拿到增益：厚、滿，整個音符一起破 |
| 5 | 47 nF | **720 Hz** | Tube Screamer 自己的值——中頻隆起型 drive |
| 10 | 4.7 nF | 7.2 kHz | 只有頂端拿到增益：在乾淨的音體上薄而兇 |

**noon 精確落在 Screamer 上是刻意的**。它給這顆旋鈕一個每個吉他手都已經知道的
參考點，也讓這顆踏板的主張可查證（§3.1）。

掃法是**幾何**的（`focus_farads(pos) = 470n · 10^(−0.2·pos)`），滑移也是幾何的：
線性滑移在小端爬得太慢、大端跳得太快；插值指數才會讓**轉角頻率**等速移動，
而那是耳朵在追的東西。

### 2.2 其餘的元件，與各自的理由

| 元件 | 值 | 理由 |
| ---- | -- | ---- |
| 削波器 | 1N4148 對級擬合，**2 正 / 1 反** | 偶次諧波撐得過中頻凹陷的 tone stack，奇次不行——而這顆踏板**就是**為了後面掛一個 stack 而設計的 |
| `C_IN` / `R_IN` | 100 nF / 1 MΩ | 耦合轉角 1.6 Hz：**刻意不做選擇**，低頻是 Focus 的地盤，前面再放一個低頻濾波會跟它打架 |
| `C_F` | 47 pF 跨在回授電阻上 | 與 Screamer 的 `C4` 同一個機制：高頻切除的轉角**隨 Drive 收緊**（154 kHz → 6.8 kHz）。沒有它，Focus 10 配高增益是無法聆聽而不是兇 |
| `R_F` | 22 k + 478 k（平方 taper） | 高頻增益 5.7× … 107× |
| `R_G` | 4.7 kΩ **固定** | 讓 Focus 掃程是純電容掃描，轉角才讀得懂 |
| Op-amp | `Ag` 3000 / `Ri` 1e9 / `Ro` 100 | 3 MHz GBW 的 JFET 對——ADR 033 的 datasheet 政策，加上 ADR 036 §3 的 `Ri` 條件數例外 |
| tone stack | **JCM800**，不是 Bassman | 凹陷較淺（7.4 dB vs 9.5 dB）、Mid 行程較大，這是一顆後面還有一台有 stack 的音箱的踏板該要的 |

### 2.3 面板與 registry

**Drive / Focus / Bass / Mid / Treble / Level**，六顆。
`controls = [Drive, Trim, Low, Mid, High, Level]`。

Focus 走 `Ctl::Trim`：連續，但它到達的是一個**電容值**而不是訊號路徑，
所以家族層不平滑它，由電路在自己的 `REBUILD = 64` 邊界滑移。

drive 家族 23 → **24 顆**。

### 2.4 這顆踏板證明了什麼

**它沒有新增任何 junction、任何 adaptor、任何 root。** 它用的是家族共用的
`NON_INVERTING_PORTS`（`ts-wdf` / `zendrive` / `king-of-tone` /
`diode-clipper` Feedback 都在上面）、既有的 `AsymDiode` root、Phase 02 的
tone stack 引擎。新的只有**安排**。

那句「這個框架已經足以拿來設計，不只是拿來移植」因此是可查證的而不是宣稱。

## 3. 驗收標準與實測

### 3.1 `cargo test`（7 條新測試，全綠）

| 測試 | 標準 | 實測 |
| ---- | ---- | ---- |
| **`focus_chooses_which_frequencies_break_up`** | 同一輸入下 Focus 10 的低頻 THD < Focus 0 的 0.4 倍；高頻 THD 仍 > 0.7 倍 | 低頻 **0.275 → 0.004（70×）**；高頻 0.266 → 0.231 |
| **`the_small_signal_gain_matches_hand_solved_ac_analysis`** | 7 個頻率對手解 AC 分析 < 1.5 % | **worst 0.22 %** |
| `focus_at_noon_is_screamer_territory` | 750 Hz 的**超額**增益是高頻的 0.66–0.82 倍（`1/√2` 的意思） | 0.742（47 Hz 2.22× / 750 Hz 21.9× / 6 kHz 29.2×） |
| `the_focus_sweep_is_monotone_at_the_bottom` | Focus 降低 → 低頻增益單調上升，底端 > 8× | 通過 |
| `the_clipper_is_asymmetric` | 剛進削波時 even/odd > 0.3 | **0.433**（THD 0.253） |
| `the_stage_is_a_circuit_and_is_well_behaved` | `memory` > 0.05 且 > 100× 曲線底；有界；**精確**靜音 | memory **1.52**（曲線底 2.2e-7） |
| `sweeping_focus_mid_note_stays_continuous` | 演奏中把 Focus 從 10 甩到 0，最大單步 < 峰值的 25 % | 0.097 / 1.222 |

**兩條是這顆的關鍵測試。**

`focus_chooses_which_frequencies_break_up` 是這顆踏板的整個論點：同一個低音 E、
同樣的輸入電平，Focus 0 的 THD 是 0.275，Focus 10 只有 0.004——**70 倍**——
而同時高頻兩邊都在破。memoryless 曲線做不出這件事，因為它的失真量與頻率無關
（`knee_shift` 對 `tanh` 回傳**恰好** 1.000000）。

`the_small_signal_gain_matches_hand_solved_ac_analysis` 是家族的既定協定
（PRD 032 / 033），而這次它又抓到同一個坑：**第一版手解參考漏了削波器的
零偏阻抗**，七個頻率一致差 1.6 %。那個「常數偏移」就是它的簽名。
`Is·(1/vt_f + 1/vt_r)` 的倒數是 7.6 MΩ，跨在 141 kΩ 的回授電阻上吃掉 1.8 % 增益。
加進去之後對到 **0.22 %**。

> 同一個坑在 ADR 035 §3 被記過、在 PRD 032 §3.1 被踩過。這是第三次，所以它
> 現在同時寫在食譜的 §4.2 與 §6 的卡點對照表裡。

### 3.2 `cargo bench`

| Bench | 中位數 | ÷ screamer |
| ----- | ------ | ---------- |
| `drive_screamer_4x_oversampled` | 31.5 µs | 1.00（校準） |
| `drive_red-charlie_4x_oversampled`（memoryless + JCM800 stack） | 31.5 µs | 1.00 |
| `drive_zendrive_4x_oversampled`（同 junction，**omega 閉式** root） | 41.0 µs | 1.30 |
| `drive_ts-wdf_4x_oversampled`（同 junction，omega root） | 42.2 µs | 1.34 |
| `drive_sd1_4x_oversampled`（簡單樹，**非對稱 Newton** root） | 68.0 µs | 2.16 |
| **`drive_mane_4x_oversampled`** | **77.9 µs** | **2.47** |

**成本歸因**：貴的不是 junction，也不是 tone stack。同一個 junction 配 omega
閉式 root 是 41 µs；`red-charlie` 證明 JCM800 stack 幾乎免費。差額是
**`AsymDiode` 的迭代 Newton root**——`sd1` 用簡單得多的樹卻付了 68 µs，就是這個。
PRD 022 的 Wright omega 閉式解只涵蓋**對稱** `sinh` 對；非對稱情形目前沒有閉式解。

77.9 µs = 64-frame deadline 的 **5.8 %**（`rangemaster` 是 12 %）。
「替非對稱 root 找閉式解」是一個記錄在案的效能工作項，不屬於本 Phase。

### 3.3 電平與抗鋸齒

預設旋鈕 **+0.03 dB**（`MAKEUP = 0.178`）。alias floor **−45.05 dB**，釘在 −40
——WDF 半邊裡數一數二乾淨的（`ts-wdf` −46.1、`big-muff` −34.4、`rangemaster` −24.1）。
`C_F` 的迴路低通幫了大忙。ADAA 在這裡**不適用**：非線性是解出來的電路（PRD 024 的前提）。

### 3.4 耳朵（**待使用者驗收**）

- **Focus 掃全程，Drive 固定在 7**，彈一個低把位 power chord：Focus 0 應該整團一起
  糊掉，Focus 10 應該低音幾乎乾淨、只有撥弦瞬間撕裂。這是這顆踏板存在的理由。
- **Focus 5 對 `ts-wdf`**：兩者應該在同一個國度（同樣的增益腳轉角、同樣的 junction），
  差別是 `mane` 的非對稱削波與後面的 JCM800 stack。
- **Bass/Mid/Treble 的互動**：推 Mid 應該同時改變高頻的落點，不是三條獨立的帶。
- 兩顆音色控制**方向不同**是設計的核心：Focus 改「破什麼」，stack 改「聽到什麼」。
  若聽起來像同一件事的兩個旋鈕，回報——那表示 Focus 的行程需要重新分配。

## 4. 非目標

- 不模擬 9 V 電源軌削頂（延續 ADR 034 §4 / ADR 035）。
- 不做 Focus 的分段選擇（它是連續的電容，就讓它連續）。
- 不追 alias floor 以外的抗鋸齒手段——解出來的電路不適用 ADAA。
- **不宣稱它像任何一顆現有踏板**。它不是移植。

## 5. 已知取捨

- **`Ag` 是單一常數**，畫不出 op-amp 6 dB/oct 的滾降（ADR 032：R-type netlist 裝不下
  電抗元件）。頂端一個八度的迴路增益比實際器件高。與 `ts-wdf` 同一個取捨。
- **`Ri` 用 1e9 而不是 datasheet 的 1e12**，理由是散射解的條件數（ADR 036 §3），
  寫在常數旁邊。
- **成本 2.47× screamer**，來源是非對稱 root 沒有閉式解（§3.2）。
- **元件值是設計選擇，不是某台實機的量測**。這顆踏板不存在於世界上——
  這正是它的意義。

## 6. 產出

- `crates/lh-dsp/src/drive/mane.rs`（新）
- `crates/lh-dsp/src/drive/mod.rs`：`FAMILY` / `MODELS` / `MODEL_COUNT` 23 → 24、
  alias bounds 追加
- `crates/lh-core/src/preset.rs`：`DRIVE_PEDALS` 23 → 24
- `app/lion-heart/src/gui/theme.rs`：`MANE` livery（茶褐色的獅鬃——家族裡唯一
  不是別人的琴箱顏色的一個）
- `docs/tone_revolution/cookbook.md` §7 的走完全程對照
