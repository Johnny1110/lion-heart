# ADR 033: op-amp 模型參數政策 + 可選二極體 UX

狀態：**已採納（2026-07-28）**
關聯：`docs/tone_revolution/phase/04-opamp-overdrive-family.md`（來源計畫）、
PRD 026（`ts-wdf` 落地規格）、ADR 032（WDF 可組合框架＝地基）
影響範圍：`drive::ts_wdf` 及**本 Phase 後續所有 op-amp overdrive**
（`zendrive`／`mxr-dist`／`rat`／`king-of-tone`／`diode-clipper`）、
`drive::Ctl`／`Circuit`、`blocks::wdf::{rtype, diode}`

## Context

Phase 04 要把整個 op-amp overdrive 家族以白箱搬進來。計畫 §1 的移植協定把
「元件值以參考實作佐證」與「二極體參數以 SPICE 代表值起步」寫得很清楚，也警告過
「BYOD 擬合參數不可盲抄」（ZenDrive 的 P1/P3 案）。第一顆 `ts-wdf` 做下來，發現
**還有兩類參數不在「元件值」的保護傘下**，而且兩類都會影響後面每一顆：

1. **op-amp 的 `Ag`/`Ri`/`Ro`**——它們不是電路上的零件，是**模型參數**。
2. **二極體選單的參數化方式**——選單要帶幾個數字，決定了「換二極體」這個動作
   在物理上是否成立。

兩者都需要一條政策，而不是一顆一顆臨場判斷。

## Decision

### 1. op-amp 模型參數以 datasheet 為準，不繼承參考實作

`ts-wdf` 採 **`AG = 3000`、`RI = 5 MΩ`、`RO = 75 Ω`**（JRC4558 典型值：
GBW ≈ 3 MHz ⟹ 1 kHz 開迴路增益約 3000）。參考實作的 `(100, 1 GΩ, 0.1 Ω)` **不
沿用**。

理由是**可測量的**，不是品味問題。`Ag = 100` 大約是 4558 開迴路增益掉到 **30 kHz**
的值；當成常數用在整個吉他頻段，實測會同時壓掉本顆的兩個招牌：

| 現象 | `Ag = 100` | `Ag = 3000` | 計畫要求 |
| --- | --- | --- | --- |
| drive 掃程頂端 | 要 117× 只給 54× | 幾乎跟到理想 | — |
| `C4` 高頻衰減（`cranked/open`） | 0.77（幾乎沒動） | ~0.60 | §4.1「drive 轉大變暗（51pF）」 |

第二列是關鍵：Phase 04 自己的驗收標準要求這個現象存在，而 `Ag = 100` 會把它糊掉。
**驗收標準優先於參考實作的參數選擇。**

**政策**：後續每一顆的 `Ag`/`Ri`/`Ro` 都從該電路實際使用的 op-amp 型號的 datasheet
取（1 kHz 附近的開迴路增益、典型輸入/輸出阻抗），並在該顆的模組文件裡寫出型號與
取值理由。參考實作的數字當交叉驗證，不當真值——**與元件值的規則相同，理由更強**：
元件值至少還是電路上量得到的東西。

**誠實界定**：常數 `Ag` 不可能到處都對，真實 op-amp 以 6 dB/oct 下滑。要正確模擬
主極點，junction 內部得能放電抗元件，而 ADR 032 的 `JEl` 只有 `Res` 與 `Vcvs`。
**最高一個八度因此被模擬成比實物多的迴路增益。** 兩條可行的後路（junction 支援
內部電容；或把主極點拉到 junction 外面當一個 port）都留給後續，等哪顆踏板真的
需要再做。有限增益模型仍然在做事：偏離理想 `1+Zf/Zg` 的量是**算出來**而非假設，
且隨要求增益成長（`the_shortfall_from_ideal_grows_with_demanded_gain`），後面迴路
增益更低的幾顆會靠這套機制。

### 2. 二極體選單帶 `(Is, n)`，不只 `Is`

一顆二極體的轉角需要**兩個**數字：`Is` 決定何時開始導通，`n`（ideality）決定導通
後電流爬多快，而 `v ≈ n·Vt·ln(i/Is)` 把兩者混在一起。**鍺不是「換了 `Is` 的矽」**
——它是高 `Is` **且** 接近 1 的 `n`，這個組合才把轉角放在 ~0.3 V 而非 ~0.6 V。

參考實作的選單只帶 `Is`，把 ideality 折進使用者面板的「# Diodes」旋鈕，並給
`1N34 → 200 pA`。配上矽的 `n`，那個值讓**鍺**檔位削得比矽**還高**——與它命名的
零件相反；而且比流通的 1N34A SPICE 模型（`IS = 2.0e-7`）小 1000 倍，看起來像
nano/pico 的單位滑手。這與 ZenDrive P1/P3 屬同一類：**參考實作的可疑處要查證，不
要繼承。**

本專案的表（`ts_wdf::DIODE_MODEL`）：

| 檔位 | `Is` | `n` | 來源 |
| --- | --- | --- | --- |
| `1N4148` | 4.352 nA | 1.906 | 原廠 TS 的 **pair-level** 擬合（吸收了實際配對的失配與體電阻） |
| `GZ34` | 2.52 nA | 1.75 | 一般小訊號矽（`screamer` 已在用） |
| `1N34` | 200 nA | 1.28 | 流通的 1N34A SPICE 模型 |
| `LED` | 1e-16 | 2.0 | 紅光 LED，~1.5 V@1 mA 的**量級擬合**，非 datasheet 萃取（已標明） |

`the_diode_selector_moves_the_knee_the_right_way` 把順序釘住：鍺 < 矽 < LED。

### 3. 「二極體數」是連續的，且與 ideality 分離

`DiodePair::set_params(is, n, vt)` 的 `n` 收的是 `count · n_device`。Count 旋鈕
（0.3–3.0，預設 1.0）縮放的是**熱電壓** `m·n·Vt`，而那是連續量——1.5 不是一顆半
二極體，是介於一顆與兩顆之間的轉角。這與參考實作把 ideality 藏進 count 的做法不
同：這裡 count 只做 count，型號只做型號，兩個旋鈕各自意思明確。

### 4. 面板路由：`Ctl::Trim` + `Circuit::set_trim`

計畫 §2.7 預期「擴充 `Ctl` 是純內部改動、零 schema 影響」，成立。
`Ctl::Shape`（stepped，既有）給 Diode；新增 `Ctl::Trim`（連續、不走家族 smoother）
給 Count，配一個 `Circuit::set_trim` 預設 no-op hook。

為什麼 Count 不走家族 smoother：它**到不了逐 sample 路徑**，只到一個係數。所以由
電路自己在子區塊邊界以 ~10 ms 一階 glide 走（`the_count_knob_glides_instead_of_
stepping` 釘住「會動、但不會一個 block 內到位」）。切二極體**型別**是 stepped，
就讓它 step——把 `Is` 跨三個數量級 glide 過去是虛構的物理。

### 5. 可選二極體只上新 key

**不對既有踏板加參數**（會動到既有 faceplate / plugin id 語意）。連帶推論，而且是
一次性的：**新踏板的面板出貨即定案**，因為 append-only 規則同樣禁止事後對既有 key
加參數。所以 Diode/Count 是 `ts-wdf` 落地**當下**必須決定的事。

## Consequences

**好的**

- 後續五顆有現成的參數政策，不必每顆重新辯論；每顆只要查一次 datasheet。
- 二極體選單在物理上站得住，順序有測試釘住。
- `Ctl::Trim` 是通用 hook：任何「連續但只到係數」的設定值都能用（偏壓、器件配對
  失配、之後的 op-amp 型號選擇都可以走它）。
- 兩處對參考實作的偏離都留下了**可測量的**理由，不是主張。

**要付的**

- 常數 `Ag` 在最高一個八度偏樂觀（見 §1 誠實界定）。
- 每顆多一道查 datasheet 的工，而且 op-amp 型號不見得每張 schematic 都標得清楚；
  標不清楚時取同級典型值並在模組文件寫明是推定。
- 新踏板的面板決策不可逆。

**沒做的**

- **junction 內部電抗元件 / op-amp 主極點**——`JEl` 仍只有 `Res` + `Vcvs`。
- 二極體的**體電阻 `Rs`** 與**結電容**：`DiodePair` 是純 Shockley 對。1N34A 這類
  點接觸鍺的 `Rs`（5–30 Ω）因此被 `n` 吸收，而不是獨立建模。
- 非對稱選單（`AsymDiode` 已存在，但 `ts-wdf` 的原廠拓撲是對稱對）。
