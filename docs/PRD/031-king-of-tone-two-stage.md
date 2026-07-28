# PRD 031: `king-of-tone` — 兩級串聯，家族唯一的「柔性」回授削波（Phase 04 第六顆）

狀態：**已實作（2026-07-28）— 待使用者耳朵驗收**
日期：2026-07-28
里程碑：Tone Revolution · Phase 04（`.../phase/04-opamp-overdrive-family.md` §2.3）
關聯：ADR 032（WDF 框架）、ADR 033（op-amp 參數政策）、PRD 026（`ts-wdf`，對照組）
新增 ADR：**無**

## 1. 背景與決策

家族其他每一顆都選一種削波方式。這顆**兩種都做，而且有先後**：一個反相 op-amp 級，
其回授裡的二極體**藏在一個電阻後面**，接著一個單純的並聯削波器。兩個 root，
每個過取樣 sample 解兩次。

### 1.1 二極體前面那顆電阻就是這顆踏板

`ts-wdf` 把二極體直接跨在回授電阻上，所以它們一導通就**成為**回授，增益塌到接近
unity。這裡削波支路是 `R11`（6.8 kΩ）**串聯**二極體，整條支路再與 `R10`（220 kΩ）
並聯。導通的二極體因此只把回授阻抗拉到約 6.8 kΩ，**不是拉到零**：這一級削波時仍保有
約三十分之一的增益，而不是全部失去。

那就是「透明 overdrive」的全部把戲，而它是一顆電阻。膝部漸進、這一級永遠不會完全
壓扁、音頭活得下來——這就是它所繼承的 Bluesbreaker 家族被買單的理由。
`the_series_resistor_keeps_the_gain_from_collapsing` 對照 `ts-wdf` 把它釘住。

### 1.2 模式開關就是換二極體

真實踏板就是這樣，所以這裡也是：

| Mode | 第一級回授 | 第二級 |
| --- | --- | --- |
| **Boost** | 削波支路抬離迴路 | 旁通 |
| **Overdrive** | 每向兩顆 1N4148 串聯 | 並聯對 |
| **Dist** | 每向一顆——clamp 減半，所以更壓縮 | 並聯對 |

Boost 的「抬離」以一個 `Is = 1e-24` 的器件表示：root 的 clamp 落在 4 V 附近，在音訊
範圍內那條支路就是開路，也就是開關做的事。

## 2. 規格

第一級 netlist 用**家族共用 junction**（`NON_INVERTING_PORTS`），但注意**訊號接在
哪個 port**：進的是 **gain leg** 而非 input leg，所以這一級是**反相**的——同一個
junction，一個字都不用改。

`R9=10k`、`C7=0.1µ`、`R_bias=1M`（接地）、`R10=220k`、`R11=6.8k`、`RL=1M`；
第二級 `R12=1k`。二極體：1N4148 SPICE 代表值 `2.52e-9 / 1.75`，Overdrive 模式每向
兩顆（`n = 3.5`）。op-amp 依 ADR 033 為**推定**同級典型值（3 MHz GBW、JFET 輸入）。

```rust
type GainLeg    = Series<Resistor, CapacitiveVoltageSource>;   // 訊號從這裡進
type OpAmpNode  = RType<4, 3, (Resistor, GainLeg, Resistor)>;
type Stage1Tree = Series<Resistor, Parallel<OpAmpNode, Resistor>>;  // R11 串 (junction ‖ R10)
// 第二級：ResistiveVoltageSource(R12) + DiodePair —— 一個 one-port 加一個 root
```

面板：**Drive / Mode / Tone / Level**。

**範圍界定（設計選擇，非元件事實）**：真實踏板在這一級之前還有一個線性增益級（另一顆
op-amp，自己的雙支路 RC leg），Drive 電位器在那裡。我們不建那一級，改讓 **`R10` 當
Drive 電位器**，掃 22 kΩ 到原廠的 220 kΩ。同一個控制、少一級，計畫 §3 非目標允許；
但沒有重現的是真實電位器對前級低頻轉角的影響。

## 3. 驗收與實測（lh-dsp 418 → 424）

| 測試 | 標準 | 實測 |
| --- | --- | --- |
| `the_linear_response_matches_hand_solved_ac_analysis` | Boost 模式下對照手解**反相**放大器 `−Ag·Zf/(Zf+Zg+Ag·Zg)`，3 drive × 4 頻率 | 12 組全數 < 3 % |
| `the_series_resistor_keeps_the_gain_from_collapsing` | 第一級退 12 dB 的存活比 < 0.6，且 < 0.8× `ts-wdf` 的 | 通過 |
| `the_three_modes_are_three_pedals` | Boost < 2 % 諧波；OD > 5× Boost；Dist > 1.3× OD | 通過 |
| `the_modes_still_differ_at_the_output` | 三模式在**踏板輸出**兩兩相對差 > 5 % | 通過 |
| `both_roots_track_the_newton_oracle` | 兩個 root 都對照 oracle | 通過 |
| 靜音（3 模式）／狂推（3 模式 × 2 端點）／多 rate | 家族慣例 | 通過 |

AC 分析這條同時確認了 **port 指派**：這是非反相踏板用的同一個 junction，訊號改插在
gain leg 上。若那個對調錯了，量到的就不會是 `−Zf/Zg`。

**機制測試必須量在第一級，不是踏板輸出**，這點值得記：第二級是硬並聯削波器，跑整顆
會把一切壓到同一個地方，正好蓋掉第一級存在的理由。第一版兩條機制測試都因此只差
1–3 %（0.732 vs 0.682、0.214 vs 0.212）而失敗——不是模型錯，是**量錯地方**。拆出
`stage1_step` / `stage2_step` 後，同樣的主張分別是 0.51 vs 0.68 與 Dist > 1.3× OD。

電平 **+0.01 dB**（`MAKEUP = 0.228`），alias floor −34.4 dB（釘 −30）。

## 4. 非目標／取捨

- **不建前置線性增益級**（見 §2 範圍界定）。
- Boost 模式的「開路」是以極小 `Is` 近似，不是真的把支路移除。
- 4.5 V 偏壓不建模（同 ADR 034 §4）。
- op-amp 型號是推定的（ADR 033 已定政策）。

## 5. 產出

`crates/lh-dsp/src/drive/king_of_tone.rs`；registry / `DRIVE_PEDALS` /
theme livery 追加；`docs/benchmarks.md`。
