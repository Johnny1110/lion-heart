# 食譜：從一張電路圖到一顆綠燈踏板

> Tone Revolution · Phase 08 §2.4（PRD 035）。
> 這份文件的驗收標準是**你（或別人）照著走得完**。走不完的地方請回報，
> 那是文件的 bug 不是你的。

面向兩種人，其實是同一件事：

- **移植一顆名踏板**——手上有 schematic，想把它變成 `drive` 家族的一員。
- **自研一顆自己的踏板**——腦中有個聲音，想用電路做出來。

範例踏板 **`mane`**（`crates/lh-dsp/src/drive/mane.rs`，PRD 036）就是照這份
食譜從零走完的，全程沒有動框架一行。遇到卡住時去讀它。

---

## 0. 先決定：這顆該不該用 WDF

**這是最重要的一步，錯了後面全白做。** ADR 035 把界線寫成一句話：

> **單端口非線性 → WDF root；多端口非線性 → 節點求解。**

| 你的電路 | 走哪條 | 範例 |
| -------- | ------ | ---- |
| 一個（或一組並聯的）二極體、一個 MOSFET 接成二極體 | **`blocks::wdf`**，二極體當 root | `screamer`、`ts-wdf`、`rat`、`mane` |
| 兩個以上互相耦合的非線性（BJT、真空管、回授對） | **`blocks::transistor`** 或自寫節點 Newton | `rangemaster`、`fuzz-face` |
| 放大器可以化簡成一個增益常數 + 回授削波 | `blocks::transistor::ShuntFeedbackStage` | `big-muff`、`sd1` |
| 完全被動、線性（音色網路） | **`eq::tonestack`**，寫 netlist 就好 | `bassman`、`jcm800`、`big-muff` |
| 沒有電抗元件，就是一條靜態曲線 | `blocks::waveshaper`（ADAA 抗鋸齒） | `ts9`、`classic` |

判斷法：**WDF root 的定義是單端口**——樹對它呈現一個入射波 `a` 和一個
阻抗 `R`，它只要在自己的 `v–i` 曲線上解一條標量方程。一顆 BJT 有兩個共用
基極的指數接面，那是雙端口，**型別就不合**，不是難度問題。硬塞會浪費你一週。

---

## 1. 寫下 netlist —— 一次，用 Rust

**沒有 `tools/netlists/`，也沒有 codegen 步驟。** 計畫初稿要求跑 R-Solver
產生符號散射矩陣；ADR 032 把它換掉了：矩陣現在**在執行期由 junction 的
netlist 數值構造**，所以 netlist 本身就是唯一的真相來源，而它是 Rust。

### 1.1 能用 series/parallel 化簡的部分：直接組樹

```rust
use crate::blocks::wdf::{Capacitor, DiodePair, Parallel, ResistiveVoltageSource, Wdf};

// 源 —2.2k— 節點 —22n— 地，二極體對掛在節點上
let mut tree = Parallel::new(
    ResistiveVoltageSource::new(2200.0),
    Capacitor::new(22e-9, os_rate),
);
let mut root = DiodePair::new(2.52e-9, 1.75, 0.02585); // (Is, n, Vt)
```

樹是**擁有式泛型**：`Parallel<A, B>` 擁有兩個子節點，單態化成直線程式碼，
零配置。型別會長，用 `type` 別名收乾淨（見 `mane.rs` 的 `ClipTree`）。

### 1.2 化簡不掉的部分：寫一個 `Junction`

Op-amp 回授、橋接網路——series/parallel 表達不了的，用 R-type。你要寫的是
**節點與埠**，不是矩陣：

```rust
static OPAMP: [JEl; 3] = non_inverting_els(AG, RI, RO); // Ri、受控源、Ro
static JUNCTION: Junction = Junction {
    nodes: NON_INVERTING_NODES,   // 5：地/+輸入/−輸入/輸出/內部節點
    els: &OPAMP,
    ports: &NON_INVERTING_PORTS,  // [(3,2) up, (1,0), (2,0), (3,0)]
};
```

**先看框架有沒有現成的佈局。** 目前有兩組，都是同一個非反相放大器，差別在
adapt 哪個埠：

| 常數 | up port | 什麼時候用 |
| ---- | ------- | ---------- |
| `NON_INVERTING_PORTS` | `(3,2)` 回授路徑 | 削波在**回授迴路裡**（TS、ZenDrive、King of Tone、`mane`） |
| `NON_INVERTING_OUT_PORTS` | `(3,0)` 輸出 | 削波在**輸出對地並聯**（Distortion+、RAT） |

**規則：up port 放非線性所在的位置。** root 只能掛在 up port 上。

五顆踏板共用這兩組佈局，任何地方都沒有寫下一個散射矩陣——這是刻意的。
真的需要新 junction 時，照 `rtype.rs` 的 `JEl`/`Junction` 寫節點就好；矩陣會
在 `calc_impedance()` 裡從你的 netlist 解出來（`N+1` 次小型 MNA，旋鈕率，
不是每 sample）。

**注意 up port 的條件**：adapt 就是令 `R_up = R_戴維南`，所以 up port 不能放在
阻抗被回授壓到近乎零的節點（op-amp 的輸出腳本身）。放在電路真的有阻抗的地方。

---

## 2. 元件參數從哪來

**授權紅線先講**（CLAUDE.md）：拓撲、元件值、器件型號是**事實**，可以用；
BYOD 是 GPL-3，**程式碼一行都不能抄**；散射矩陣任何來源都不轉抄。

### 2.1 二極體與電晶體：`(Is, n)` 兩個數字，缺一不可

ADR 033 的政策：**選單帶 `(Is, n)`，不是只帶 `Is`**。膝部電壓是
`v ≈ n·Vt·ln(i/Is)`，兩個參數混在一起決定。鍺不是「`Is` 不同的矽」——它是
**高 `Is` 且 `n` 接近 1**，那個配對才是它膝部落在 0.3 V 而不是 0.6 V 的原因。
只換 `Is` 會做出「鍺比矽晚導通」這種反物理的結果（ADR 033 記了一個真實案例）。

現成可用的數字，直接抄本專案自己的表：

| 器件 | `Is` | `n` | 出處 |
| ---- | ---- | --- | ---- |
| 1N4148（單顆 SPICE） | 2.52e-9 | 1.75 | `screamer`、`sd1`、`diode-clipper` |
| 1N4148（**對**級擬合） | 4.352e-9 | 1.906 | `ts-wdf`、`mane`——吸收了實際配對誤差與體電阻 |
| 1N34A 鍺 | 2.0e-7 | 1.28 | `ts-wdf` 的 Diode 選單 |
| 紅色 LED | 1.0e-16 | 2.0 | 數量級擬合，非 datasheet |

**自己的器件**：`tools/fit_device.py` 會從 datasheet 的 I–V 點擬出 `(Is, n)`：

```sh
python3 tools/fit_device.py --points 0.5,1e-4 0.6,1e-3 0.7,1e-2
python3 tools/fit_device.py --csv my_diode.csv --show-residuals
python3 tools/fit_device.py --pair --points 0.5,1e-4 0.6,1e-3   # 擬「一對」而不是單顆
```

它只需要 numpy/scipy，**不需要 SPICE**——計畫 §2.2 原本要 ngspice 跑暫態再擬合，
但擬一條指數 I–V 曲線本來就不需要模擬器，而 Phase 02 早就把 ngspice fixtures
換成本專案自己的節點分析 oracle 了。

### 2.2 Op-amp：`Ag`/`Ri`/`Ro` 來自 datasheet，不准編造

也是 ADR 033。三個數字是**模型參數**不是電路元件，所以不能從別人的實作繼承。
兩個實務提醒：

- `Ag` 用**音頻帶**的開迴路增益，不是 DC 值。一個 R-type netlist 裝不下電抗
  元件，所以單一受控源畫不出 6 dB/oct 的滾降；填 DC 增益會讓 5 kHz 的迴路增益
  高出 60 dB。3 MHz GBW 的 op-amp 在 1 kHz 是 `Ag ≈ 3000`。
- `Ri` 大到某個程度就沒差了。JFET 輸入的 1e12 Ω 與 1e9 Ω 在 junction 裡不可分辨
  （兩者都遠大於其他所有阻抗），但 1e12 會把散射解的條件數推到 `f32` 撐不住。
  用 1e9，並把理由寫在常數旁邊（`mane.rs` 就是這樣寫的）。

---

## 3. 拼出踏板

一顆 drive 踏板要實作 `Circuit`，四個方法：

```rust
impl Circuit for MyPedal {
    fn prepare(&mut self, base_rate: f32, os_rate: f32);  // 樹在 os_rate 離散化
    fn shape(&mut self, block: &mut [f32], drive: &[f32]); // 非線性，過取樣率
    fn post(&mut self, block: &mut [f32], tone: &[f32]);   // 線性收尾，基頻率
    fn eq(&mut self, block: &mut [f32], low: &[f32], mid: &[f32], high: &[f32]); // 選配
}
```

每 sample 的骨架**永遠是這五行**，不管樹多複雜：

```rust
self.set_input(x);                                  // 驅動樹裡唯一的源
let a = self.tree.reflected();                      // 收集反射波
let (_v, b) = self.diode.solve(a, self.tree.resistance()); // 解 root
self.tree.incident(b);                              // 推回去
self.output()                                       // 讀一個埠電壓
```

### 3.1 旋鈕：分清楚「進訊號路徑」與「只到係數」

| 旋鈕種類 | `Ctl` | 平滑在哪 |
| -------- | ----- | -------- |
| 進每 sample 路徑（Drive 前級增益、Tone） | `Ctl::Drive` / `Ctl::Tone` / `Ctl::Level` | 家族的 `Smoothed`，逐 sample 軌跡 |
| 只到係數（電阻值、電容值、二極體數量） | `Ctl::Trim` | **電路自己**在 `REBUILD` 邊界滑移 |
| 分段選擇（器件型號、拓撲） | `Ctl::Shape` / `Ctl::Mode` | 不平滑，直接跳（換器件是 setup 手勢） |
| 被動音色網路三段 | `Ctl::Low/Mid/High` | 走 `eq()` → `eq::tonestack` |

`Ctl::Shape/Trim/Mode` 是 `lh-dsp` 私有的，**追加不動 preset/plugin schema**。

**settled-skip 慣例**：只有旋鈕真的動了才重建阻抗。

```rust
fn retune(&mut self, drive_pos: f32) {
    let ohms = feedback_ohms(drive_pos);
    if ohms != self.fb_ohms {
        self.fb_ohms = ohms;
        self.tree.port1_mut().set_ohms(ohms);
        self.tree.calc_impedance();   // 後序遞迴整棵樹，一次
    }
}
```

`calc_impedance()` 是**旋鈕率**的事，`REBUILD = 64` 個過取樣 sample（192 kHz 下
是 3 kHz 的重建率，遠高於任何手部動作）。電容的 state **就是**電路的電壓，
所以重建之間狀態原封不動帶過去——這就是掃旋鈕不會爆音的原因，不需要交叉淡化。

### 3.2 一個非顯然的細節：非線性掃法要配合物理量

`mane` 的 Focus 掃的是一顆跨兩個十倍頻程的電容，所以它**用幾何方式滑移**：

```rust
let ratio = self.focus_target / self.focus;
if (ratio - 1.0).abs() > 1e-4 {
    self.focus *= ratio.powf(self.glide);   // 不是 self.focus += d * glide
    ...
}
```

線性滑移在小端爬得太慢、大端跳得太快；插值指數才會讓**轉角頻率**等速移動，
而那是耳朵在追的東西。

### 3.3 即時安全（不可協商）

`shape` 跑在音訊執行緒上。CLAUDE.md 的規則全部適用：不配置記憶體、不上鎖、
不 syscall、迭代次數有上限、denormal 要 flush。`blocks::wdf` 的 root 已經替你
處理了阻尼與迭代上限；你要負責的是**別在 `shape` 或 `reset` 裡做任何配置**。

`reset()` **也跑在音訊執行緒上**。要做昂貴的一次性計算（例如解直流工作點），
放在 `prepare()`，把結果存起來，`reset()` 只還原狀態——`rangemaster` 是範例。

---

## 4. 驗證：三層，由外而內

這是 Phase 08 真正的新東西。**特性測試（character test）證明不了電路是對的**
——「有中頻隆起」「有偶次諧波」「膝部隨頻率移動」，一個錯得很像的電路也做得到。

### 4.1 第一層：對照獨立的節點分析（最強的一層）

`lh_dsp::testutil::netlist` 是一個**完全獨立**的 MNA 求解器：梯形伴隨模型 +
阻尼 Newton，與 `blocks::wdf` 不共用任何程式碼、公式或常數。把你的電路**寫兩次**
——一次是 WDF 樹，一次是 netlist——然後比對。

```rust
use lh_dsp::testutil::netlist::{Circuit, El, GND};

let els = [
    El::Src { node: 1 },
    El::R { a: 1, b: 2, ohms: 2200.0 },
    El::C { a: 2, b: GND, farads: 22e-9 },
    El::Pair { a: 2, b: GND, is: 2.52e-9, vt_n: 1.75 * 0.02585 },
];
let mut refc = Circuit::new(&els, 3);
refc.prepare(192_000.0);
```

**兩邊用同一個梯形離散化，所以它們是同一個離散系統的兩種寫法**，不是同一個
連續系統的兩種近似。因此該對到算術精度，而殘差是有意義的：

| 比對 | 實測殘差 | 殘差是什麼 |
| ---- | -------- | ---------- |
| 串並聯樹，靜態轉移曲線 | **2.5e-7 V** | `f32` 的底 |
| 串並聯樹，動態（Newton root） | **1.5e-7 V** | 同上 |
| 串並聯樹，動態（omega 閉式 root） | **2.1e-4 V** | **Wright omega 的近似誤差**，比樹本身大 1400 倍 |
| R-type junction，靜態 | 相對 **3.2e-5** | `f32` 在條件數幾百的散射解裡的表現 |

最後一列值得記住：junction 裡的元件值跨七個數量級（`Ri` = 1 GΩ 旁邊
`Ro` = 100 Ω），條件數就上去了。3e-5 是 −90 dB，聽不到，但它是**你能對到的
上限**，比這更嚴的容差是在測浮點數不是測電路。

`tests/whitebox.rs` 是完整的範例。

### 4.2 第二層：手解 AC 分析

削波門檻以下電路是線性的，所以增益有封閉解。這是本家族的既定作法
（PRD 032/033/036），而且它抓得到第一層抓不到的東西——**元件值本身寫錯**。

**一定要記得算削波器的零偏阻抗。** 反並聯二極體對在 0 V 附近不是開路，它的
斜率是 `Is·(1/vt_f + 1/vt_r)`。`mane` 的例子：那是 7.6 MΩ，跨在 141 kΩ 的回授
電阻上，吃掉 **1.8 %** 的增益。漏掉它，你的手算會和模型差 1.6%，而且七個頻率
差得一模一樣——那個「常數偏移」就是它的簽名。加進去之後 `mane` 對到 **0.22 %**。

（同一個坑在 ADR 035 §3 裡被記過一次，在 PRD 032 裡被踩過一次。第三次了。）

### 4.3 第三層：白箱判別

`lh_dsp::testutil::whitebox` 把「這到底是不是電路」做成了可複用的量測。餵它一個
`FnMut(f32) -> f32`，它回你一個數字：

| 函式 | 問的問題 | 曲線的值 | 電路的值 |
| ---- | -------- | -------- | -------- |
| `memory` | 同一個瞬時輸入會不會給出不同輸出？ | ~1e-6 | 0.05 起跳（`mane` 1.52） |
| `knee_shift` | 失真量會不會隨頻率變？ | 恰好 1.000000 | `mane` 的 shunt 對照組 0.355 |
| `harmonics(...).even_over_odd()` | 對稱還是不對稱？ | — | `mane` 0.43 |
| `bounded` | 灌 ±1e6 V 會不會發散？ | — | 必須有限 |
| `silent` | 靜音進去是不是**精確**靜音出來？ | — | 必須是 |

`memory` 是最鋒利的一個，理由值得懂：**memoryless 波形整形是一個函數**
`y = f(x)`，同一個 `x` 必須給同一個 `y`；一個有電抗的電路根本不是 `x` 的函數，
它的輸出取決於電容上的電荷。所以用兩個互質頻率去驅動它，讓每個輸入位準被從
各種歷史狀態造訪到，再看輸出的散布。

（實作細節：分桶之後要**先擬合二次式再看殘差**。只減線性項會把曲線的曲率算成
記憶——`tanh` 在 1/128 寬的桶裡就有 2e-4，足以蓋過一個弱電抗電路。）

**用在削波級上，不要用在整顆踏板上**：曲線後面接一個濾波器也會有記憶，
那是濾波器的，不是非線性的。

### 4.4 還有兩件家族要求的事

- **`assert_no_alloc` 離線閘門**：`crates/lh-dsp/tests/alloc.rs` 會自動掃到你的
  新踏板（它遍歷 `FAMILY.pedals`），不用手動加。
- **鋸齒地板**：`drive/mod.rs` 的 `alias_floor_survey`（`#[ignore]`）量，然後把
  數字釘進同檔的 bounds 表。**解出來的電路不適用 ADAA**（PRD 024 的前提是顯式
  曲線），4× 過取樣是唯一防線。實測範圍：`mane` −45 dB、`big-muff` −34 dB、
  `rangemaster` −24 dB（家族最高）。

---

## 5. 進 registry（**只能追加**）

四個地方，順序無所謂，漏掉會有測試抓你：

1. `crates/lh-dsp/src/drive/mod.rs`：`mod mypedal;`、`FAMILY.pedals` 追加
   `&mypedal::DESC`、`MODELS` 追加 `ModelDef`、`MODEL_COUNT` +1。
2. `crates/lh-core/src/preset.rs`：`DRIVE_PEDALS` 追加 key，陣列長度 +1。
3. `app/lion-heart/src/gui/theme.rs`：一個顏色，`match pedal_key` 追加一行。
4. `drive/mod.rs` 測試裡的 alias bounds 表追加一列。

**為什麼只能追加**：preset 以**位置**存踏板索引、以 **key** 存參數值；plugin 的
參數 id 是 `{slot}_{pedal_key}_{param_key}`。所以在中間插入會錯位所有舊檔，
但**在既有踏板上追加一顆旋鈕是安全的**（舊檔缺那個 key → 取預設值）。
`fuzz-face` 從 2 顆變 3 顆就是這樣做的（PRD 034 §2.2）。

最後跑一次閘門：

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test --workspace --release     # AllocDisabler 在 release 下會被編掉，兩個都要跑
cargo bench -p lh-dsp --bench effects -- "drive_mypedal"
```

---

## 6. 常見卡點

| 症狀 | 多半是 |
| ---- | ------ |
| 手算與模型差幾個 % 而且**每個頻率差一樣多** | 漏了削波器的零偏阻抗（§4.2） |
| `calc_impedance()` 之後有爆音 | 你在重建時順手 `reset()` 了樹。不要——狀態就是電容電壓，要帶過去 |
| root 發散或輸出 NaN | up port 放在阻抗趨近零的節點（op-amp 輸出腳）。移到有真實阻抗的地方 |
| 靜音進去不是精確靜音出來 | 電路裡有直流偏壓。PRD 032 §2.3 為了保住這個性質刻意不建偏壓網路 |
| debug build 直接 SIGABRT（exit 134） | `shape`/`reset` 裡配置了記憶體。這是即時違規不是崩潰 |
| 掃某個旋鈕時聲音「卡卡的」 | 物理量掃法不對（§3.2），或 `REBUILD` 太大 |
| 高增益時刺得無法聽 | 回授電阻上沒有並聯小電容。真踏板都有一顆（TS 的 `C4` = 51 pF） |

---

## 7. 走完全程的範例

`mane`（PRD 036）從頭到尾用的就是這份文件：

- **§0** 判定：單一二極體堆疊 → WDF root。✅
- **§1** 沒寫新 junction，直接用 `NON_INVERTING_PORTS`。
- **§2** op-amp 用 datasheet、二極體用 `ts-wdf` 已有的對級擬合。
- **§3** Focus 掃增益腳的電容（`Ctl::Trim`，幾何滑移）；Bass/Mid/Treble 走
  `eq()` 進 JCM800 被動網路。
- **§4** 七個頻率對手解 AC 分析（0.22 %）、`memory` 1.52 對曲線的 2.2e-7、
  even/odd 0.43、鋸齒地板 −45 dB。
- **§5** registry 四處追加。

它證明的是這句話：**這個框架已經足以拿來設計，不只是拿來移植。**
`mane` 沒有新增任何 junction、任何 adaptor、任何 root。
