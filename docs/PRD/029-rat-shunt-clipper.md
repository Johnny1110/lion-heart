# PRD 029: `rat` — 增益超過 op-amp 能力的並聯削波（Phase 04 第四顆）

狀態：**已實作（2026-07-28）— 待使用者耳朵驗收**
日期：2026-07-28
里程碑：Tone Revolution · Phase 04（`.../phase/04-opamp-overdrive-family.md` §2.5）
關聯：PRD 028（`mxr-dist`，同一個 output-adapted 佈局）、ADR 033（op-amp 參數政策）、
ADR 032（WDF 框架）
新增 ADR：**無**

## 1. 背景與決策

家族共用放大器的第三顆、**output-adapted 佈局**（`NON_INVERTING_OUT_PORTS`）的第二顆，
所以電路工作只剩選零件。不共用的是：**這顆跑得比它的 op-amp 還遠。**

### 1.1 刻意讓 op-amp 不夠用

開到底時 gain leg 約 76 Ω、回授 100 kΩ，電阻要求 **1300×**；LM308 在 1 kHz 大約只有
1000 的開迴路增益。**迴路增益因此小於 1**——這一級根本閉不上迴路，實際輸出約 570×，
且隨頻率繼續掉。

這不是要道歉的模型不足，而是 RAT 之所以聽起來像 RAT、而不像很大聲的 Distortion+ 的
主要原因。理想 op-amp 模型會老實給出 1300×，然後在一個明顯、可聽的方向上錯掉。
`the_stage_runs_out_of_op_amp` 把這個落差釘住，並要求它**隨要求增益成長**。

### 1.2 其餘的性格

- **gain leg 是兩條 RC 支路，不是一條**：`47Ω+2.2µF` 並聯 `560Ω+4.7µF`，兩個轉角
  差約一個半八度，所以增益是**分兩階**爬過低頻的。那個階梯就是 RAT 的低音，家族裡
  沒有第二顆單轉角踏板有。
- **Filter 是反的，而且那是對的**：真實旋鈕是低通的串聯電阻，**往上轉 = 更暗**。
  本 crate 每一個其他 tone 控制都相反。照真實踏板做，因為那是 RAT 使用者的手記得的
  方向——並在模組文件與面板名稱上寫明。
- **並聯削波經一個串聯 RC 到地**，矽，硬，而且前面那麼多增益，幾乎立刻就到。

## 2. 規格

Netlist（`NON_INVERTING_OUT_PORTS`，子 port 依序為回授／輸入腳／gain leg）：
`C1=22n`、`R2=1M`（偏壓，接地）、`R3=1k`、`C2=1n`、`R4=47Ω`、`C5=2.2µ`、
`R5=560Ω`、`C6=4.7µ`、`Rdist=100k`、`C4=100p`、`R6=1k`、`C7=4.7µ`。

```rust
type InputLeg  = Parallel<Series<Parallel<CapacitiveVoltageSource, Resistor>, Resistor>, Capacitor>;
type GainLeg   = Parallel<ResistorCapacitorSeries, ResistorCapacitorSeries>;
type OpAmpNode = RType<4, 3, (ResistorCapacitorParallel, InputLeg, GainLeg)>;
type ClipTree  = Series<OpAmpNode, ResistorCapacitorSeries>;
```

op-amp（ADR 033）：**LM308**，`AG=1000`（1 MHz GBW）、`RI=4e7`（super-beta 輸入）、
`RO=75`。二極體：矽對，`Is=5e-9 / n=2.0`。

面板：**Dist / Filter / Volume**，真實踏板的三顆，不加。4.5 V 偏壓不建模（同 ADR 034 §4）。

## 3. 驗收與實測（lh-dsp 399 → 409）

| 測試 | 標準 | 實測 |
| --- | --- | --- |
| `the_linear_response_matches_hand_solved_ac_analysis` | 手解 AC 分析，**5 dist × 5 頻率** | 25 組全數 < 2 % |
| `the_stage_runs_out_of_op_amp` | 開到底實測 < 0.75× 理想；且落差隨要求增益成長 | 通過 |
| `the_gain_leg_climbs_through_the_bass_in_two_steps` | 20/60/200/600/2000 Hz 單調上升，且跨度 > 8× | 通過 |
| `the_closed_form_root_tracks_the_newton_oracle` | 閉式 vs oracle | 通過 |
| `the_filter_knob_darkens_as_it_turns_up` | Filter 10 的轉角 < 0.1× Filter 0 | 通過 |
| 靜音／狂推／多 rate | 家族慣例 | 通過 |

**AC 分析這條在本顆多做一件事：把 `Ro` 留在算式裡。** 家族其他顆都把它省掉（回授會把
它除以迴路增益），但這顆的迴路增益掉到 1 以下，那個論證就不成立了。改用兩條節點方程
手解：`vo = Ag·vp / [ (1 + Ag·β) + Ro(1−β)/Zf + Ro/Zl ]`。

**oracle 測試的度量再修一次**（`mxr-dist` 已從相對殘差改為 oracle 比對）：oracle **自身**
的收斂要以「**隱含電壓誤差**」`residual / (df/dv)` 表示，不是原始殘差。這裡 port 電阻
上千歐姆、前面又有幾百倍增益，root 深在指數區、`df/dv` 上看 10⁶——數十微伏的殘差其實
是**皮伏**級的電壓誤差，直接界定殘差會誤判一個已收斂的解算器。而能斷言的下限是
**f32**：`solve_newton` 內部用 f64、回傳 f32，半伏附近的間距約 6e-8 V，所以 1e-6 是
有餘裕的真實界，再緊就是在量回傳型別。

電平 **+0.18 dB**（`MAKEUP = 0.128`），alias floor −30.2 dB（釘 −26；與 `mxr-dist`
同因：先放大幾百倍再削平）。

## 4. 非目標／取捨

- 不建 4.5 V 偏壓。
- **常數 `AG` 的限制在這顆最嚴重**：真實 LM308 的增益隨頻率下滑，而常數模型讓高頻
  保有比實物多的迴路增益，所以高頻的「跟不上」比實物少。要修得讓 junction 支援電抗
  內部元件（ADR 033 已記為後續）。
- **不模擬轉速率限制（slew rate）**。LM308 的 0.15 V/µs 是 RAT 音色的一部分，而 WDF
  的線性元件模型不含它。這是本顆最大的已知缺口，且無法用調參數補。

## 5. 產出

`crates/lh-dsp/src/drive/rat.rs`；registry / `DRIVE_PEDALS` / theme livery 追加；
`docs/benchmarks.md`。
