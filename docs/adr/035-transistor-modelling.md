# ADR 035: 電晶體級的建模法 — 三條路各走各的，NDK 列未來研究

狀態：**已採納（2026-07-29）**
關聯：`docs/tone_revolution/phase/05-fuzz-transistor-family.md`（來源計畫）、
PRD 032（`big-muff`）、PRD 033（`rangemaster`）、PRD 034（`fuzz-face` 型號選擇）、
ADR 032（WDF 可組合框架）、ADR 033（元件參數來自 datasheet 的政策）
影響範圍：新增 `lh_dsp::blocks::transistor`；`drive::{big_muff, rangemaster, fuzz_face}`
**並修正計畫兩處**：ADR 編號（計畫暫定 032，實為 **035**）、以及參考實作的兩個數值問題（§3）

## Context

Phase 01–04 建立了一條很好用的路：電路 → WDF 樹 → 單一非線性 root → 標量解。
Phase 04 六顆 op-amp overdrive 全部落在這條路上，五顆還共用同一個 junction。

Phase 05 這一族**走不進去**，而且不是「比較難」，是**三種不同的走不進去**。計畫 v2
已經看出這一點（把初稿「統稱 WDF 電晶體 root」推翻），本 ADR 把它固定成決策，並補上
落地時才查得出來的兩件事。

WDF root 的前提是：線性子網路能歸約成一棵樹，樹對 root 呈現**一個**戴維南等效
（一個入射波 `a`、一個 port 阻抗 `R`），root 只要在自己的 v–i 曲線上解一條標量方程。
電晶體電路違反這個前提的方式各不相同：

| 電路 | 為什麼不是 WDF root |
| ---- | ------------------- |
| Big Muff 削波級 | 削波的是**回授二極體**，這部分完全可以是 WDF。真正的問題是那顆「op-amp」是線性化成 `A = −Rc/Re` 的共射級——它**沒有可宣稱的輸入/輸出阻抗**，而 R-Type junction 正需要那兩個數字（ADR 033 才剛把「不准編造 op-amp 參數」寫成政策） |
| Rangemaster | 非線性**就是電晶體本身**：兩個共用基極的指數接面 = **雙端口**非線性。WDF root 的定義是單端口，這不是難度問題，是型別不合 |
| Fuzz Face | 兩顆電晶體強耦合回授對，四個接面互相牽制。可解，但要的是完整的節點狀態空間（DK 法），那是一整套框架 |

## Decision

### 1. 新開 `blocks::transistor`，而不是硬塞進 `blocks::wdf`

兩個東西，各對應上表的前兩列：

- **`ShuntFeedbackStage`** — 固定增益反相放大器 + 回授網路（R ‖ C ‖ 反並聯二極體），
  以**標量阻尼 Newton** 解一條節點方程。放大器把和節點釘在 `y/A`，所以回授網路兩端
  的電壓恰好是 `κ·y`（`κ = 1 − 1/A`），KCL 收成

  ```
  F(y) = y/A − u − R_th·i(κ·y) = 0
  i(v) = 2·Is·sinh(v/nVt) + v·G_f + (v·G_c − s_c)
  ```

  `A < 0` 且 `di/dv > 0` ⟹ `F′ < 0` 恆成立，根唯一、Newton 不會停在駐點。
- **`Bjt`** — Ebers–Moll 傳輸型電晶體。它**不解方程**，只提供某工作點的
  **線性化（companion）**：三個端電流 + 六個跨導。多節點 Newton 由呼叫端跑
  （`drive::rangemaster` 的三節點）。附 `solve3`（3×3 部分主元高斯消去；節點電導跨十個
  數量級，主元選擇不是可選項）。

兩者沿用 WDF root 已經立下的慣例：`f64` 內部、逐 sample warm start、**阻尼**（步長以
熱電壓為單位設上限）、固定迭代上限（RT 規則 1、7）。

**為什麼不放 `blocks::wdf`**：那個模組的整份文件在講波變數與樹；這裡一個波都沒有。
放進去會讓「WDF」這個詞失去意義，而這個詞現在是專案裡最有價值的一個抽象。

### 2. 三顆踏板，三條路

| 踏板 | 走法 | 理由 |
| ---- | ---- | ---- |
| **`big-muff`**（PRD 032） | `ShuntFeedbackStage` ×2 串接 | 與 `sd1` 同機制類。**不需要任何新求解機制**——這是本 Phase 最有價值也最可行的一顆 |
| **`rangemaster`**（PRD 033） | `Bjt` + 三節點 Newton | 唯一真正需要器件模型的一顆。單電晶體、三個未知數，範圍可控 |
| **`fuzz-face`**（PRD 034） | **維持 behavioral**，加型號選擇 | 見 §4 |

### 3. 參考實作的兩個數值問題（落地時量出來的）

計畫把 BYOD 當教科書讀是對的——**拓撲與元件值是事實**。但這一族的兩份參考碼各有一個
問題，都在本專案自己重推方程時才浮出來。照 CLAUDE.md「若程式與文件不符就標記出來」
記在這裡。

#### 3.1 Big Muff：回授電流注入在**直流**戴維南電阻上（差一個數量級）

BYOD 的節點方程是 `y = A·(u + R20·i_fb)`，其中 `u` 是輸入高通的輸出、`R20` = 100 kΩ。

輸入網路是「源 —C5— R19 — 和節點，R20 對交流地」。它的**開路電壓**確實是
`s·C5·R20 / (1 + s·C5·(R19+R20))`——BYOD 的輸入濾波器係數正是這個，所以拓撲判讀無誤。
但同一個網路的**戴維南電阻**在音頻是 `R19 ‖ R20 = 9.09 kΩ`，不是 `R20 = 100 kΩ`；
100 kΩ 是它在**直流**（`C5` 開路）的值。BYOD 用了直流電阻配交流電壓。

後果可量：閉迴路增益 `≈ 1/(1/A − κ·R_th/R_f)`，

- `R_th = 9.09 kΩ` → **−28.0×**（含二極體零偏阻抗）
- `R_th = 100 kΩ` → **−4.3×**

**6.5 倍**。一顆叫 "sustain" 的踏板，每一級少 6.5 倍增益不是小數點問題。本專案用交流值，
並由 `the_linear_response_matches_hand_solved_ac_analysis` 對手解 AC 分析釘住——那條測試
與實作零共用推理，用錯值會差 6 倍、離 3 % 容差十萬八千里。

#### 3.2 Rangemaster：輸出縮放是**繪圖的殘留**，器件參數是矽的

BYOD `RangeBooster.cpp` 的回傳值是

```cpp
return ((i_c * RV) * 1e16 - 5e5 - 1.0);
```

同一組魔數 `* 1e16 - 5e5 - 1.0` 一字不差地出現在 `sim/Rangemaster/rangemaster.py`
的 **`plt.plot(...)` 那一行**。`1e16` 之所以必要，是因為那份程式對一顆 **PNP** 餵
NPN 的 Ebers–Moll 方程（`v_be = v_b − v_e ≈ −0.54 V`），整顆電晶體被算在逆向區，
`i_c` 落在 1e-16 A 量級。之後再補 `+54 dB` 與 `tanh` 才有聲音。

它會發出聲音，而且大概是好聽的聲音——但那不是 Rangemaster 的物理，**不能當參考實作**。
於是本專案自行推導：PNP 極性、三節點 KCL、C1/C3 梯形伴隨模型，工作點在 `prepare` 解一次
（PRD 033 §2.3）。

器件參數同樣不照抄：BYOD 用 `Is = 10 fA / βF = 200`，那是**矽**。鍺 OC44 的 `Is` 在
1e-7 A 量級——七個數量級——而那正是鍺會停在 0.2 V `Vbe`、軟導通、會漏、會漂的原因。
這是 **ADR 033 的政策往器件層延伸一格**：二極體選單帶 `(Is, n)` 而不是抄來的 `Is`，
電晶體同理帶自己的 `(Is, βF, βR)`。工作點由
`the_operating_point_is_where_germanium_puts_it` 用四行手算釘住。

### 4. Fuzz Face 維持 behavioral，NDK 列**未來研究**

BYOD 的 `FuzzFaceNDK` 由 `ChowDSP/Research/NDK-Framework` 產生，**該工具未公開**。
沒有可移植的公開產生器；從文獻（Holters/Zölzer 的 DK 法）自建是一整套框架的工作量，
而且與本 Phase 其他兩顆共用不了任何東西。

**決策**：現有 behavioral `fuzz-face` 保留（它已經抓到三大特徵：非對稱削波、gated
decay、cleans up），本 Phase 只做計畫 §2.2 允許的「小幅精修」——加一個
**Germanium / Silicon** 型號選擇（PRD 034）。兩組 voicing 的每一項差異都從器件推出來
（β 高 3–5 倍 → 增益；`Vbe` 可預測、漏電可忽略 → 偏壓靠近中點 → 對稱、偶次諧波變薄；
不漏電 → **沒有 blocking distortion，所以沒有 gate**；不 woolly → 亮）。

NDK 正式列為**未來研究線**（overview 的深水區候補）。若日後要做，前提是自建產生器並
**完全不參考 BYOD 的 NDK config 與程式碼**（GPL-3）。

### 5. 授權紅線的執行紀錄

- 沒有移植任何 BYOD 程式碼。
- 用到的只有：拓撲、元件值、器件型號——CLAUDE.md 明列的「事實」。
- 方程是教科書 Ebers–Moll 傳輸模型與普通節點分析；離散化是梯形（雙線性）伴隨模型。
  兩者都不是任何人的著作。
- §3 的兩個問題是**本專案自己重推後量出來的差異**，不是「照抄後發現」。

## Consequences

**好的**

- 「WDF」在這個 codebase 裡繼續只表示一件事。想加電晶體級的人有第二個模組可去，
  而兩個模組的邊界有一句話能說清楚：**單端口非線性 → root；多端口 → 節點求解**。
- `ShuntFeedbackStage` 是可複用的：任何「固定增益反相級 + 回授削波」都能直接用
  （Muff 的四種年代版本、muff 系衍生踏板、自製電路）。
- `Bjt` + `solve3` 是 Phase 08 自研平台的第一塊電晶體積木。三節點 Newton 的寫法在
  `rangemaster` 裡有一份可抄的範例（19 行）。
- 參考實作的兩個問題被量化記錄，下一個讀 BYOD 的人不會再踩。

**要付的**

- **`rangemaster` 是家族最貴的一顆**：163 µs / 64-frame block（≈ 4.6× `screamer`，
  ~12 % of deadline）。三節點 Newton × 2 個 exp × ~3 次迭代 × 192 kHz 就是這個價。
  收斂只要 2 步（實測），所以省不下迭代；要再快得換 branch-free 的 3×3 解與更省的
  指數近似，那是效能專項工作，不在本 Phase。
- **兩顆新踏板的鋸齒地板偏高**（`big-muff` −34 dB、`rangemaster` −24 dB）。非線性是
  「解出來的電路」不是「顯式曲線」，**ADAA（PRD 024）不適用**；4× 過取樣是唯一防線。
  `rangemaster` 是家族最高的一個地板。
- `ShuntFeedbackStage` 把共射級簡化成一個增益常數，所以**電晶體自己的非線性、
  `re` 隨電流變化、電源軌削頂都沒有**。Muff 的削波全部來自二極體——真實踏板大致如此，
  但推到極端時會少一層層次。
- `fuzz-face` 的面板從 2 顆變 3 顆。preset 以 **key** 存參數、plugin id 是
  `{slot}_{pedal_key}_{param_key}`，所以追加是安全的（舊檔缺 `type` → 取預設
  Germanium = 既有行為，character 測試不動）。

**沒做的**

- 沒有做 NDK / DK 法。
- 沒有做電源軌（9 V）削頂，也沒有建 Big Muff 的直流偏壓——延續 ADR 034 §4 的簡化。
- 沒有做 Big Muff 的年代變體（Triangle / Ram's Head / Sovtek 元件值不同）。框架已經
  支援：`ShuntFeedbackStage::new` 收全部元件值，加一個 stepped 選擇器即可。
- 沒有做鍺電晶體的**漏電與溫漂**（`Ico`）。那是鍺 fuzz 會隨天氣變聲的原因，
  也是 Phase 08 想玩的人的好題目。
