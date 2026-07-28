# PRD 030: `diode-clipper` — 一顆二極體，四種接法（Phase 04 第五顆／平台件）

狀態：**已實作（2026-07-28）— 待使用者耳朵驗收**
日期：2026-07-28
里程碑：Tone Revolution · Phase 04（`.../phase/04-opamp-overdrive-family.md` §2.6）
關聯：ADR 032（WDF 框架）、ADR 033（`(Is, n)` 二極體慣例、`Ctl` 擴充政策）
新增 ADR：**無**（`Ctl::Mode` 是 ADR 033 §4 已鋪好的路的第二個用例）

## 1. 背景與決策

家族裡其他每一顆都是某一顆特定的踏板。這一顆是**框架本身，加上旋鈕**：選拓撲、
選器件、選幾顆，在其他一切固定的情況下聽每個選擇做了什麼。

存在理由有二。它是**誠實的 A/B 台**——拿 `ts-wdf` 比 `mxr-dist`，同時混進了削波器
位置與十幾個元件值的差異；這裡只有接法在變。它也是**給人照著做的範例**（包括作者
自己）：四棵樹、兩個 root，以及那個觀察——**root 與樹是各自獨立的**，Rectify 就是
Shunt 的樹換一個非對稱 root，其餘一字不改。

計畫 §2.6 把這顆定位為「教學件 + 自研起點，最能展示 Phase 08 平台」。

## 2. 規格

### 2.1 四種接法

| Circuit | 樹 | Root | 做什麼 |
| --- | --- | --- | --- |
| **Shunt** | 源 ‖ 電容 | 對稱對 | 經典款；電容讓削波門檻隨頻率移動 |
| **Series** | 源串聯負載 | 對稱對 | **死區**——小訊號根本推不開二極體，所以它是唯一「彈輕會變乾淨」而不是「變髒」的接法 |
| **Rectify** | 源 ‖ 電容 | 非對稱，2 正 / 1 反 | 兩半削在不同高度 → 偶次諧波 |
| **Feedback** | 家族的 op-amp junction | 對稱對 | 二極體對抗的是放大器而非電阻，這就是 op-amp overdrive 柔的原因 |

**四棵樹同時存在**，切換只是寫一個 enum——不配置、不重建，而且每棵樹各自保有狀態，
所以切換是兩個已穩定電路之間的接續，不是一段暫態。

元件值刻意平凡（一個好讀的電阻、一個整數電容），且**不宣稱是任何真實踏板**。
本家族裡只有這一顆是這樣，而那正是重點。

### 2.2 面板：Drive / Circuit / Diode / Count / Tone / Level

`Ctl::Mode` + `Circuit::set_mode` 為此新增——第二個 stepped hook，與 `Ctl::Shape`
同契約（ADR 033 §4 已把這條路鋪好：`Ctl` 是 `lh-dsp` 私有，擴充零 schema 影響）。

器件表 `(Is, n)`：`Si` `2.52e-9/1.75`、`Ge` `2.0e-7/1.28`、`LED` `1e-16/2.0`。
Count 連續 0.3–3.0，電路內以 ~10 ms glide。

## 3. 驗收與實測（lh-dsp 409 → 418）

| 測試 | 標準 | 實測 |
| --- | --- | --- |
| `all_four_wirings_sound_different` | 兩兩相對差 > 5 % | 通過（6 組） |
| `rectify_adds_even_harmonics_that_shunt_does_not` | Shunt 的二次諧波 < 1 %，Rectify > 10× 之 | 通過 |
| `the_series_wiring_has_a_dead_zone` | 輸入退 26 dB 後電平比 < 0.05（比例是 0.05），且 Shunt > 3× 之 | 通過 |
| `the_shunt_cap_makes_break_up_frequency_dependent` | Shunt 高頻破音 < 低頻/1.2；且無電容的 Series 更平 | 通過 |
| `the_device_controls_move_the_knee` | Ge < 0.8× Si < LED/1.5；Count 3.0 > 1.5× Count 0.5 | 通過 |
| `switching_wiring_mid_note_stays_bounded` | 每 512 sample 換一次接法，全程有限有界 | 通過 |
| 靜音／狂推（4 接法 × 3 器件） | 家族慣例 | 通過 |

**兩件在寫測試時才發現、值得記的事：**

1. **Series 模式的輸出取樣一開始是錯的。** 我原本寫 `e − v`（源電壓減二極體壓降）。
   但 series adaptor 在 root 呈現的是 **`−e`**，所以 `v` 已經是負的壓降——`e − v`
   實際上是**加**上壓降，死區整個消失，而測試一開始只是「比例不夠低」而非明顯爆炸。
   正解是從 **port 電流**取：`i = (a − v)/R`，輸出 `= (v − a)·R_load/R_total`。
   凡是輸出不在 root 節點上的拓撲，都該這樣取而不是手推。
2. **頻譜洩漏會蓋過要量的效應。** 二次諧波測試原本用 220 Hz，在 192 kHz 下不是整數
   週期，基頻洩漏進諧波 bin 的量（1.3 %）比 Shunt 真正的二次諧波還大。改用
   **187.5 Hz = 192 kHz / 1024**，任何 2 的冪次視窗都是整數週期，Shunt 立刻掉到
   0.1 % 以下。

電平 **+0.04 dB**（`MAKEUP = 0.157`），alias floor −36.1 dB（釘 −32）。

## 4. 非目標／取捨

- **不是任何真實踏板**，元件值不宣稱來自任何電路。
- Feedback 模式用的是家族共用 junction 配平凡零件，不是第五顆名機。
- 切換接法會有一個階躍（各樹狀態獨立），這是設定動作而非演奏動作。

## 5. 產出

`crates/lh-dsp/src/drive/diode_clipper.rs`；`blocks::wdf::AsymDiode::set_params`
（可選器件的非對稱對應）+ `AsymDiode` 保存支路數；`drive::Ctl::Mode` +
`Circuit::set_mode`；registry / `DRIVE_PEDALS` / theme livery 追加。
