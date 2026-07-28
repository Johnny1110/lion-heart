# PRD 028: `mxr-dist` — 輸出端削波的 op-amp distortion（Phase 04 第三顆）

狀態：**已實作（2026-07-28）— 待使用者耳朵驗收**
日期：2026-07-28
里程碑：Tone Revolution · Phase 04（`docs/tone_revolution/phase/04-opamp-overdrive-family.md` §2.4）
關聯：PRD 026 / ADR 033（`ts-wdf`、家族參數政策）、PRD 027 / ADR 034（`zendrive`）、
PRD 025 / ADR 032（WDF 框架）
新增 ADR：**無**（沿用 ADR 032 / 033 的既有決策；本顆唯一的新結構是框架內既有機制的
第二種佈局，見 §2.1）

## 1. 背景與決策

前兩顆把二極體放在**回授迴路裡**，op-amp 與它角力，結果是一個柔和、協商出來的膝部。
這一顆完全相反：**先放大**（最高 213×），再把結果丟給**並聯到地**的二極體對。
沒有協商——這一級是「一個限幅器前面掛了很多增益」，而那就是 Distortion+ 聽起來
硬、壓縮，而 Screamer 聽起來只是在「靠上去」的原因。

結構上，這件事表現為**哪一個 port 被 adapt**：up port 必須面對非線性，所以本顆用
**`NON_INVERTING_OUT_PORTS`**——同一個放大器、同樣四個 port，但輸出接出去、回授電阻
降格成一般子 port。

### 1.1 「adapt 哪個 port」現在是框架層的一個選擇

`blocks::wdf` 新增 `NON_INVERTING_OUT_PORTS`（與既有的 `NON_INVERTING_PORTS` 並列，
共用 `non_inverting_els`）。兩者是**同一個放大器的兩種 adapt 方式**，文件寫明選擇
準則：*非線性在哪裡，up port 就在哪裡*。Screamer 系削波在迴路內 → 前者；
Distortion+ / RAT 削波在輸出 → 後者。`rat` 接下來可直接沿用。

**ADR 032 §5 的退化警告在這裡不適用**，值得寫清楚：adapt 在輸出端等於問「這個節點
的戴維南阻抗」，而回授下的 op-amp 輸出很硬——`Ro/(1+Ag·β)`，此處 0.15–13 Ω。小，
但定義明確，而且被後面的 `R5`（10 kΩ）完全淹沒。ADR 032 踩到的退化是**阻抗掉到求解
器地板以下**，那需要一個理想化的 `Ro`（BYOD 用 0.1 Ω）；照 ADR 033 用 datasheet 的
75 Ω 就不會發生。

## 2. 規格

### 2.1 Netlist

```
ports : [ (out,gnd) up=輸出→並聯削波器 , (out,−) R4 回授 , (+,gnd) 輸入腳 , (−,gnd) gain leg ]
els   : op_amp(1, 2, 3, 4, AG, RI, RO)          // 與 ts-wdf / zendrive 同一份
```

元件：`R1=10k`、`C2=10n`、`R2=1M`（偏壓，此處接地——見 §2.3）、`R3=4.7k`、
`C3=47n`、`R4=1M`、`R5=10k`、`C4=1µ`、`Rout=10k`、`C5=1n`。

```rust
type InputLeg  = Parallel<Resistor, Series<Resistor, CapacitiveVoltageSource>>;
type OpAmpNode = RType<4, 3, (Resistor, InputLeg, ResistorCapacitorSeries)>;
type ClipTree  = Parallel<Capacitor, Parallel<Resistor, Series<ResistorCapacitorSeries, OpAmpNode>>>;
```

`Rout`、`C5` 與輸出串聯支路**匯在同一個節點**，所以二極體的節點電壓**就是**跨 `Rout`
的電壓——輸出取樣不花任何成本，`step()` 直接回傳 root 解出的 `v`。

op-amp（ADR 033 政策）：**741**，`AG=1000`（1 MHz GBW @ 1 kHz）、`RI=2e6`、`RO=75`。
1970 年代的踏板配 1970 年代的零件，而**它增益不夠用這件事是聲音的一部分**：Dist 開到
底時 1 kHz 的迴路增益只剩約 5，更高頻更少，所以這一級**真的給不出**電阻要求的 213×。

### 2.2 Dist 旋鈕：以增益定義 taper

本顆的電位器在 **gain leg 裡**（不是回授裡），所以 `增益 = 1 + R4/R_leg` 對電位器是
雙曲線的——直接線性掃會把整個可用範圍擠進旋鈕最後十分之一。所以 taper 定義在**增益**
上：從 `GAIN_MIN=2` 幾何掃到 `GAIN_MAX = 1 + R4/R3 = 213`，再反解回電阻。旋鈕因此
線性於**分貝**，而兩端仍然是真實的元件值。

### 2.3 兩處沿用前一顆的簡化

- **4.5 V 偏壓不建模**（同 ADR 034 §4）：`R2` 接地而非接偏壓軌。理由相同——沒有電源軌
  的 op-amp 模型裡它是純共模。
- **輸入端的 `C1`（1 nF）未建模**：它在參考模型裡與訊號源**並聯**，而該源的內阻是
  1e-9 Ω（函式庫預設），所以它在數學上**完全無作用**。它是輸入插孔的 RF 旁路，只有
  對真實高阻抗源（拾音器）才有意義。少一個節點、少一次逐 sample 運算，換零差異。

### 2.4 面板：Dist / Diode / Output

真實踏板只有兩顆旋鈕；第三顆是**唯一值得有的版本差異**——早期單體用**鍺**
（`1N34`，`Is=2.0e-7 / n=1.28`）、後期用矽（`1N914`，`2.52e-9 / 1.75`），兩者不是
同一顆踏板。`(Is, n)` 兩參數的慣例沿用 ADR 033。預設 = 鍺（原版）。

**沒有 tone 控制**，因為真實踏板沒有；它的音色定位已經在電路裡（進去是 gain leg 的
轉角，出來是 `C5` 與輸出網路）。`post()` 只做 makeup 與 DC block。

## 3. 驗收標準與實測

### 3.1 `cargo test`（lh-dsp 389 → **399**，workspace 全綠，debug 與 release 皆綠）

| 測試 | 標準 | 實測 |
| ---- | ---- | ---- |
| `the_linear_response_matches_hand_solved_ac_analysis` | 對照手解 AC 分析（輸入高通 → 有限增益放大器 → 輸出網路），**5 dist × 5 頻率** | 25 組全數 < 2 % |
| `the_closed_form_root_tracks_the_newton_oracle` | omega 閉式 vs Newton oracle，50 000 sample | 見下 |
| `the_gain_leg_makes_it_mid_forward` | 1 kHz 增益 > 3× 的 80 Hz | 通過（10.3× vs 34.9×） |
| `it_breaks_up_where_the_zendrive_is_still_clean` | 同旋鈕同輸入下 `zendrive` 諧波 < 0.01 而本顆 > 10× 之 | 通過 |
| `it_compresses_hard` | 輸入退 12 dB 後電平比 > 0.6（線性是 0.25） | 通過（0.80） |
| `it_clips_hard_at_playing_level` | 吉他電平 + dist 8 下非基頻能量 > 0.3 | 通過 |
| `the_diode_selector_picks_the_version` | 鍺 clamp < 0.8× 矽 | 通過 |
| `silence_stays_silent` / `bounded_when_slammed` / `the_response_holds_across_sample_rates` | 同家族慣例 | 通過 |

**AC 分析那一條在這裡多做了兩件事**：它同時證明「把 junction adapt 在輸出 port」是
對的，以及「輸出網路的巢狀 series 組合確實等於 schematic 上那條扁平串聯鏈」——兩個
本顆才第一次出現的結構問題，都由一條與實作零共用推理的測試裁決。

**root 測試改了度量方式，值得記。** 原本沿用前兩顆的「相對於 `a` 的殘差」，量到
8.5e-3、超標。追下去不是解錯，是**度量選錯**：`DiodePair::solve` 是閉式*近似*，
節點電壓上有一個大致固定的絕對誤差（PRD 022 量到 ~30 µV），除以很小的 `a` 自然爆掉，
而那個比值對「聽得到的地方準不準」毫無資訊。本顆的鍺二極體又比家族其他矽的更陡
（`n·Vt` 33 mV vs 49 mV），把近似所平滑的交越區弄得更尖。改為 **PRD 022 建立的 oracle
比對**：閉式 vs `solve_newton`，並要求 oracle 本身的殘差 < 1e-6 V。

### 3.2 `cargo bench`（同一輪）

| Bench | 中位數 |
| ----- | ------ |
| `drive_screamer_4x_oversampled` | ~32.5 µs |
| `drive_zendrive_4x_oversampled` | ~41.1 µs |
| `drive_ts-wdf_4x_oversampled` | ~42.7 µs |
| **`drive_mxr-dist_4x_oversampled`** | **~48.8 µs** |

48.8 µs ＝預算 3.7 %。比前兩顆多約 15 %，因為樹更深：輸入腳多一層 `Series`，輸出網路
是三層（`Series` → `Parallel` → `Parallel`）而不是一層。

### 3.3 電平與抗鋸齒

預設旋鈕 **+0.01 dB**（`MAKEUP = 0.44`）。alias floor −34.6 dB，釘在 −30——家族裡
偏高（僅次於 `monster5150`/`red-charlie` 那些串接削波），因為這是**唯一一顆把訊號
放大 200 倍才削平**的：進到削波器的波形早就不是正弦，而 4× 過取樣擋不住方波的高階。
真要壓下去得提高過取樣率或在削波器前後補濾波——都不在本 Phase 範圍。

### 3.4 耳朵（**待使用者驗收**）

與 `zendrive` 的對照最能說明本 Phase：同樣的 op-amp、同一份 netlist 骨架，只是削波器
換了位置，聽感從「透明、跟手」變成「硬、壓縮、中頻凸」。另外聽 Diode 切鍺/矽的音量與
squareness 差異。

## 4. 非目標

- 不建 4.5 V 偏壓／電源軌削頂；不建輸入端的 RF 旁路 `C1`（§2.3）。
- 不加 tone 控制（真實踏板沒有）。
- 不追 alias floor——本顆的鋸齒地板來自「先放大 200 倍再削平」這個拓撲本身。

## 5. 已知取捨

- **`AG=1000` 是 741 在 1 kHz 的值**，常數化的限制同 ADR 033：最高一個八度被模擬成
  比實物多的迴路增益。本顆對這點特別敏感（迴路增益本來就只剩個位數），所以高頻的
  增益衰減會比實物**少**一些。
- **alias floor −34.6 dB**，家族偏高端（見 §3.3）。
- **鍺/矽只有兩檔**，沒有做 LED 之類的改裝檔——真實 Distortion+ 的改裝文化不在這裡。

## 6. 產出

- `crates/lh-dsp/src/drive/mxr_dist.rs`（新）
- `crates/lh-dsp/src/blocks/wdf/rtype.rs`：`NON_INVERTING_OUT_PORTS`（同一放大器的
  第二種 adapt 佈局）
- registry / `DRIVE_PEDALS` / theme livery 追加
- `docs/benchmarks.md`
