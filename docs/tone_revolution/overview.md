# Tone Revolution — 移植計畫藍圖（Overview）

狀態：**執行中** — **Phase 01（PRD 022）、02（PRD 023 / ADR 030）、06（PRD 024 /
ADR 031）已實作（2026-07-27）；Phase 03（PRD 025 / ADR 032）已實作（2026-07-28）**。
三個 quick win 加上架構級的 adaptor 框架完成，**Phase 04（op-amp overdrive 家族）
**已完成**：`ts-wdf`（PRD 026 / ADR 033）與 `zendrive`（PRD 027 / ADR 034）、
`mxr-dist`（PRD 028）、
`rat`（PRD 029）、`diode-clipper`（PRD 030）、`king-of-tone`（PRD 031）**六顆全數落地**，
drive 家族 15 → **21 顆**。
日期：2026-07-24（初稿）／2026-07-27（v2 校訂：對照 BYOD/chowdsp_wdf 原始碼
逐項查核技術主張，修正錯誤並補強風險；變更摘要見 §11）

里程碑：白皮書 §6 深水區研究線（WDF 白箱電路模擬）之總攻計畫；承接
PRD 020 / ADR 028（WDF TS 削波級）、PRD 021 / ADR 029（WDF 回授式 overdrive
`sd1`）
研究來源：`/mnt/BYOD`（Build-Your-Own-Distortion，ChowDSP，GPL-3）、
`/mnt/chowdsp_wdf`（Wave Digital Filter 函式庫，ChowDSP，BSD-3）——兩者同一作者
Jatin Chowdhury；`chowdsp_wdf` 正是 BYOD 的底層，也是 lion-heart `blocks::wdf`
手工重建之物的**成熟上游參考**。

> **進度**
>
> | Phase | 狀態 | 產出 |
> | ----- | ---- | ---- |
> | 01 快速非線性 root | ✅ **已實作** | PRD 022；`blocks::wdf::omega`；`screamer` 72.3 → **30.5 µs**、root 12.7×、誤差 30 µV |
> | 02 Tone stack 框架 | ✅ **已實作** | PRD 023 / **ADR 030**；`eq::tonestack`（netlist → 狀態空間 → Tustin）；3 機型；5 顆 FMV 系 drive 遷移；獨立 `tonestack` 踏板 |
> | 06 Waveshaper + ADAA | ✅ **已實作** | PRD 024 / **ADR 031**；`blocks::waveshaper`（一/二階 ADAA + 12 曲線）；**全部 12 顆 memoryless drive 抗鋸齒**（地板 −29 → −38…−87 dB）；新踏板 `waveshaper` |
> | 03 WDF adaptor + R-Type | ✅ **已實作** | PRD 025 / **ADR 032**；`blocks::wdf` 拆五檔（one-port／adaptor／rtype／diode／omega）；擁有式泛型樹；**散射矩陣執行期由 netlist 數值構造**；op-amp 有限增益；`screamer`/`sd1` 等價重寫（對改寫前 ~1e-8） |
> | 04 op-amp overdrive 家族 | ✅ **已實作**（6/6） | PRD 026 / **ADR 033**：`ts-wdf`（完整 TS 削波級 + 可選二極體 UX）；op-amp 參數改採 datasheet、二極體選單帶 `(Is, n)`；新增離線 `assert_no_alloc` 閘門。PRD 027 / **ADR 034**：`zendrive`（**與 TS 共用 junction**，提到框架層 `NON_INVERTING_PORTS`）；削波器（1N4148+2N7002 疊層）自行擬合，並**更正計畫對 BYOD 擬合值的判讀**。PRD 028：`mxr-dist`（**輸出端並聯削波**，框架新增 `NON_INVERTING_OUT_PORTS` 佈局；741 op-amp）。PRD 029：`rat`（迴路增益 < 1，Filter 反向）。PRD 030：`diode-clipper`（**平台件**——一顆二極體四種接法，新增 `Ctl::Mode`）。PRD 031：`king-of-tone`（**兩級兩 root**，柔性回授削波）。drive 家族 15 → **21 顆** |
> | 05、07–08 | 待排 | — |
>
> 四個 Phase 的實作落差都記在各自 `phase/NN-*.md` 頂端的方框裡：01 是自行擬合的
> 四次猜測、精度定性修正、latency vs throughput、branchless 反而變慢；02 是引擎
> 形式由「手推封閉式 + 三階直接式 IIR」改為「netlist → 狀態空間」、六個機型只交付
> 三個（其餘無法佐證元件值）、ngspice fixtures 換成獨立節點分析 oracle；06 是二階
> ADAA 自行從定義推導、dry-sum 補償實測後**完全不需要**（ADAA 在 4× 率上）、改造
> 範圍由 8 顆擴大到全部 12 顆 memoryless 踏板；**03 是把「R-Solver 符號 codegen」
> 這個硬前置與「數值構造」這個後備對調**（連帶 `tools/wdf_codegen/` 未產出），
> 理由與驗證管道見 ADR 032。

---

## 0. 一句話

把 lion-heart 目前「一顆一顆手寫、以 memoryless 波形整形為主」的 drive/tone，
升級成一套**可組合的白箱電路框架**——讓（1）tone stack 是真實被動網路而非圖形
EQ、（2）業界名踏板的既有設計參數能整批移植進來、（3）這套框架成為你**自研音色
踏板**的開發平台。

## 1. 願景與問題陳述

lion-heart 的 drive 家族目前 **15 顆**（`ts9`、`bd2`、`classic`、`centaur`、`evva`、
`red-charlie`、`monster5150`、`angry-charlie`、`jan-ray`、`fuzz-face`、`overdrive`、
`screamer`、`sd1`、`angry-charlie-v2`、`waveshaper`），其中僅 `screamer` / `sd1`
是 WDF 白箱，其餘皆 memoryless（自 Phase 06 起全部走 ADAA 抗鋸齒）。使用者回報
兩點不滿，兩者其實共根：

- **Tone stack 聽起來像圖形 EQ，不像音箱。** 現行 `drive::ToneStack`
  （`crates/lh-dsp/src/drive/mod.rs`）是三個**獨立、相加、互不干擾**的濾波帶
  （`x + lo·lo + mid·bp + hi·hi`）。真實 Fender/Marshall 被動 tone stack 是一個
  **耦合 RC 網路**：三個旋鈕會互相牽動，且 noon 時**天生有中頻凹陷**（招牌
  scoop）。現行版本 noon 全平、旋鈕正交——這是「不像音箱」的根源。而
  `red-charlie`/`monster5150`/`angry-charlie`/`evva` 的骨架都內建這個 stack，所以
  **修好 tone stack 也連帶修好一半的 drive 不滿**。

- **Drive 缺少真實 clipper 的「反應感」。** memoryless 是瞬時、與頻率無關、與電抗
  元件零互動的靜態曲線；真實削波的靈魂恰恰是它沒有的——RC 與二極體接面互動讓
  削波門檻隨頻率/暫態移動、回授網路隨 drive 變化、對稱性由實際二極體決定。
  `blocks::wdf` 的註解自己講得很清楚，`screamer`/`sd1` 已證明方向對，只是還沒鋪開。

**Tone Revolution 的任務**：把「方向對但零散」的白箱路線，做成「框架化、成規模、
可自研」的音色核心。

## 2. 三大核心目標（驗收此計畫的準繩）

1. **一套完美的 tone stack 框架。** 真實被動 tone stack 的互動與凹陷、涵蓋主要
   機型（Fender Bassman/Twin、Marshall JCM800、Vox AC30、Baxandall、Big Muff
   tone、James/passive），可被任何 drive 複用、也能當獨立 tone/EQ 踏板。→ Phase 02。

2. **把別人設計好的每一顆 drive 參數搬進來（我要所有的 drive）。** 業界名踏板的
   電路拓撲 + 校準過的元件值 + 擬合過的二極體/電晶體參數，整批進 lion-heart。
   → Phase 03–07（依建模技術分家族）。§5 有完整清冊。

3. **框架要能支撐我未來的自研踏板開發。** 從 netlist 到可跑的 Rust 白箱，要有
   工具鏈、擬合流程、驗證 harness 與「新增一顆 WDF 踏板」的食譜。→ Phase 08。

## 3. 架構論點：三層白箱框架

目前 `blocks::wdf` 是**手工化約**的極簡版——只有 `Capacitor`、`DiodePair`/
`AsymDiode`（Newton 解）、`parallel_root`；每個電路都得手推代數化簡成直線程式碼。
這條路對「一兩顆」可行，對「所有的 drive + 完美 tone stack + 自研平台」不夠。要
graduate 成三層：

```
┌─────────────────────────────────────────────────────────────┐
│ 第 3 層  應用：每顆 drive / 每個 tone stack                    │
│   drive::{screamer, sd1, zendrive, rat, mxr, kot, bigmuff…}   │
│   eq::tonestack::{bassman, jcm800, ac30, baxandall…}          │
├─────────────────────────────────────────────────────────────┤
│ 第 2 層  可組合 adaptor（Phase 03）                            │
│   one-ports：Resistor / Capacitor / Res±Cap / V-source        │
│   adaptors：Series / Parallel / **R-Type（散射矩陣 + op-amp）**│
│   線性 tone stack 引擎（Phase 02，解析傳輸函數，可獨立於 WDF） │
├─────────────────────────────────────────────────────────────┤
│ 第 1 層  非線性 root 求解（Phase 01）                          │
│   **Wright Omega 閉式**（取代 Newton）＋ 電晶體/真空管 root    │
└─────────────────────────────────────────────────────────────┘
```

關鍵洞見（來自研究 BYOD 全部 WDF drive）：**op-amp overdrive 家族是同一個核心**
——「WDF 樹 → op-amp R-Type 散射矩陣（op-amp 以有限增益 Ag/輸入阻抗 Ri/輸出阻抗
Ro 建進矩陣）→ 二極體 root」。ZenDrive 的散射矩陣**與 Tube Screamer 一字不差**
（只差零件值與擬合的二極體參數）。所以第 2 層一旦有 R-Type，第 3 層的 TS/SD-1/
ZenDrive/King of Tone/MXR/RAT 幾乎是「換 R/C 值 + 貼散射矩陣 + 設二極體」。

## 4. 移植來源與授權合規（**務必先讀**）

lion-heart 應用碼是 **MIT OR Apache-2.0（寬鬆雙授權）**（見 `Cargo.toml`、
`README.md`）。移植來源授權**不同**，界線必須守住，否則會污染 lion-heart 的授權：

| 來源 | 授權 | 能不能用 | 做法 |
|---|---|---|---|
| `chowdsp_wdf`（WDF 框架、R-Type、adaptor、二極體模型） | **BSD-3** | ✅ 可移植 | 以 Rust 重寫（演算法/結構），保留出處與 BSD 版權宣告 |
| `omega.h`（Wright Omega，D'Angelo） | **MIT** | ✅ 可移植 | 直接以 Rust 重寫，附 MIT 出處 |
| **BYOD 本體**（各 drive/tone 的 `.cpp`/`.h`、Surge waveshaper） | **GPL-3** | ⚠️ **不可整段搬碼** | 見下 |
| 類神經模型權重（Centaur ML、GuitarML、RONN） | 各異/常 GPL | ⚠️ 多半不可散布 | 自行訓練或找寬鬆來源；Phase 07 專章 |

**GPL 界線（BYOD）——什麼是安全的：**

- **電路拓撲、元件值（R/C、二極體型號與 SPICE 參數）＝事實**，不受著作權保護，
  可自由使用。這是「別人設計好的 drive 參數」的合法本體。
- **散射矩陣**：不要複製貼上 BYOD 產生出來的矩陣文字（灰色地帶）。**改用
  R-Solver（`github.com/jatinchowdhury18/R-Solver`）從 netlist 自己重新產生**——
  netlist 是電路圖（事實），R-Solver 的輸出是數學。Phase 08 把這條工具鏈做起來。
- **演算法/技術**（ADAA、WDF 化約、電晶體模型）可自行以 Rust 重新實作；**具體
  GPL 程式碼不可翻譯照抄**。Surge waveshaper 依數學重寫，別搬碼。

> 一句話：**框架與二極體解法從 BSD/MIT 的 `chowdsp_wdf`/`omega.h` 移植；電路的
> "設計參數" 從公開事實（元件值、SPICE model、netlist）取得；GPL 的 BYOD 只當
> 「怎麼做」的教科書，不當「複製來源」。** 每個碰到 GPL 的 Phase 檔都會重申界線。

## 5. 完整 pedal 清冊（目標 2 的範圍）

依**建模技術**分類（也就是 Phase 分家的依據）。lion-heart 已有者標註。

### 5a. op-amp + 二極體 overdrive（同一 WDF 核心，Phase 04）

| 踏板 | 原型 | BYOD 來源 | lion-heart 現況 |
|---|---|---|---|
| Tube Screamer | Ibanez TS808/9 | `drive/tube_screamer`（回授 R-Type + 可選二極體） | 有 `ts9`(memoryless)、`screamer`(WDF shunt)；忠實回授版以**新 key** `ts-wdf` 追加（Phase 04 §2.1，既有不動） |
| Boss SD-1 | Boss SD-1 | —（非對稱衍生） | 有 `sd1`(WDF 理想 op-amp)；有限增益忠實版同走新 key（Phase 04 §2.1） |
| Zen Drive | Hermida Zendrive | `drive/zen_drive`（與 TS 同矩陣、擬合 MOSFET-diode；**含移植陷阱，見 Phase 04 §2.2**） | 無（`jan-ray`=Timmy 同族，你喜歡這味） |
| King of Tone | Analog Man KoT | `drive/king_of_tone`（兩級 WDF：op-amp R-Type overdrive + 簡單樹 clipper） | 無 |
| MXR Distortion+ | MXR Dist+ | `drive/mxr_distortion`（op-amp R-Type + Ge/Si 二極體） | 無 |
| RAT | ProCo RAT | `drive/mouse_drive`（op-amp R-Type + 濾波網路） | 無 |
| Diode Clipper/Rectifier | 通用 | `drive/diode_circuits`（可組態 WDF clipper） | 無（可當「白箱通用 clipper」教學件） |

> **排除**：BYOD 原創設計（Flapjack、Warp、Blonde）不列主線——它們不是業界名機
> （不在「別人設計好的名踏板」目標內），且其電路設計本體出自 BYOD 專案，「元件值
> ＝公開事實」的論據對它們最弱。真想要再另議。

### 5b. Fuzz / 電晶體 / booster（Phase 05）

| 踏板 | 原型 | BYOD 來源 | 建模技術（v2 查核後） |
|---|---|---|---|
| Big Muff | EHX Big Muff Pi | `drive/big_muff`、`drive/muff_clipper` | **非 WDF**：固定增益 CE 電晶體級 + 回授二極體 + C12，逐 sample 標量 Newton——與 lion-heart `sd1` 的回授二極體 root 同類機制（見 Phase 05） |
| Fuzz Face | Dallas Arbiter | `drive/fuzz_machine`（`FuzzFaceNDK`） | NDK（節點 DK 法）——**BYOD 的 NDK 碼由私有工具生成，無公開產生器**；lion-heart 已有 behavioral `fuzz-face`，本計畫維持之（見 Phase 05） |
| Rangemaster | Dallas Rangemaster | `drive/RangeBooster.cpp` | 完整 **Ebers-Moll** BJT 標量 Newton（真正需要「電晶體 root」的一顆） |
| Bass Face | （Fuzz Face 低音版） | `drive/BassFace.cpp` | BYOD 原創變體——**不列主線**（同 5a 排除原則） |

### 5c. Memoryless waveshaper（Phase 06）

| 件 | BYOD 來源 | 內容 |
|---|---|---|
| Waveshaper bank | `drive/waveshaper`（Surge） | soft/hard/asym/sine/digital/fold/cheby/fuzz…數十種，**含 ADAA 抗鋸齒** |
| ~~Warp / Blonde~~ | `drive/Warp.cpp`、`drive/BlondeDrive.cpp` | BYOD 原創數位失真——**不列主線**（同 5a 排除原則） |

### 5d. 類神經 / 真空管（Phase 07，最重、部分暫緩）

| 件 | BYOD 來源 | 依賴 |
|---|---|---|
| Centaur（Klon） | `drive/centaur`（`GainStageML` + WDF 削波 + summing amp） | RTNeural 權重 |
| GuitarML Amp | `drive/GuitarMLAmp.cpp` | RTNeural（LSTM）權重 |
| RONN | `drive/RONN.cpp` | 隨機類神經 |
| Junior B | `drive/junior_b`（`ModifiedRType` + `NeuralTriodeModel`） | 類神經三極管（連白皮書「triode stage」深水題） |
| Tube Amp | `drive/tube_amp` | 真空管級 |

### 5e. Tone stack / EQ（目標 1，Phase 02）

| 件 | BYOD 來源 | 技術 |
|---|---|---|
| Bassman FMV/TMB | `tone/bassman`（WDF 6-port R-Type） | 被動 tone stack（互動 + scoop） |
| Baxandall | `tone/baxandall`（WDF） | Hi-Fi bass/treble |
| TS Tone | `tone/tube_screamer_tone`（WDF） | TS 的 tone 控制 |
| Ladder Filter | `tone/ladder_filter` | Moog 式 LP/HP ladder |

> 誠實界定：5d（類神經/真空管）是最重、且有**權重授權/資產**問題的一塊；本計畫
> 把它排在最後且標為**可選/暫緩**，不阻擋 5a–5c 的高價值主線。「我要所有的 drive」
> 在工程上先由 5a/5b/5c 兌現絕大多數，5d 視資源與授權再議。

## 6. Phase 藍圖總表

| # | Phase | 命中目標 | 依賴 | 產出 | 規模 |
|:-:|---|:-:|:-:|---|:-:|
| 01 | 快速非線性 root（Wright Omega） | 2 的成本 | — | `blocks::wdf` 加 omega 解、A/B、bench | 小 |
| 02 | **Tone stack 框架** | **1** | — | `eq::tonestack` 解析引擎 + 機型註冊表；換掉 `ToneStack` | 中 |
| 03 | WDF 可組合 adaptor + R-Type + op-amp | 2/3 地基 | 01 | 第 2 層框架；新 ADR | 大 |
| 04 | op-amp overdrive 家族 | 2 | 03 | TS/SD-1/ZenDrive/KoT/MXR/RAT/DiodeClipper + 可選二極體 | 大 |
| 05 | Fuzz/電晶體/booster 家族 | 2 | 01(部分) | BigMuff/Rangemaster 白箱 + fuzz-face 精修 | 中 |
| 06 | Waveshaper bank + ADAA | 2 + 品質 | — | waveshaper 踏板 + 既有 drive 抗鋸齒改造 | 中 |
| 07 | 類神經/真空管家族（可選） | 2 | 神經路徑 | Centaur/GuitarML/triode… | 大 |
| 08 | **自研平台工具鏈** | **3** | 03 | netlist→R-Solver→codegen、SPICE 擬合、驗證 harness、食譜 | 中 |

**建議執行順序與理由（v2 調整）：**

1. **01 → 02 → 06**：三個彼此獨立的 quick win，全部直接改善**既有**聲音——
   01 Wright Omega（讓 WDF 從「奢侈品」變「日常」，為 03/04 鋪路）、02 tone stack
   （命中最明確的不滿、連帶改善多顆 FMV 系 drive）、06 ADAA（既有 memoryless
   drive 全面去毛躁，完全不依賴 WDF 框架）。06 提前的理由：它與 02 合起來正面
   回應「drive 不滿意」的兩個根源（tone stack + 高把位毛躁），且風險最低。
2. **03（內含 08 的 R-Solver 最小 bootstrap，見 Phase 03 §2.5）**：架好可組合
   adaptor + R-Type + op-amp——這是「所有 op-amp drive」與「自研平台」的共同
   地基。**架構級改動，需新 ADR。** R-Solver 工具鏈必須在此階段先跑通，否則
   03/04 的散射矩陣沒有乾淨來源（授權紅線）。
3. **04 → 05**：依家族鋪開 drive（04 op-amp 家族一次一顆 PR；05 縮減後的
   fuzz/電晶體家族）。
4. **08**：框架穩定後把工具鏈補完（SPICE 擬合、食譜、自研範例踏板），交付「自研」
   能力。
5. **07**：最後、可選；連結白皮書 triode 深水題與 ADR 027 跨平台。

每個 Phase 的**具體工作內容**見 `phase/NN-*.md`。

## 7. 跨階段共同決策（每個 Phase 都適用）

- **RT 規則不可破**（CLAUDE.md §即時音訊規則）：audio thread 上零配置、零鎖、
  無 syscall；WDF 樹在 `prepare` 建好；迭代/矩陣維度上界固定；denormal flush；
  非有限輸出在 debug build assert。新踏板一律過 `assert_no_alloc`。
- **Append-only**：新踏板追加進 `MODELS`/`DRIVE_PEDALS`（`ModelDef` = desc +
  `Ctl` routing + build fn），**盡量不 bump preset schema**；plugin 由
  `from_families` 自動展開參數（append-only ⇒ 既有 plugin 參數 id 不變；追加後
  **重跑 clap-validator**）。註：`Ctl` routing enum 是 `lh-dsp` 內部私有型別，
  不進 preset/plugin——擴充它（新踏板需要的 Voice/選擇器等）**零 schema 影響**。
- **升級既有 vs 新增**：`screamer`/`sd1`/`fuzz-face` 的「忠實版」以**新 key 追加**
  （保 preset/plugin id 穩定），或在 ADR 明確記錄為 append-only。Tone stack 例外——
  它是**共用建構塊**，換掉會改變既有 FMV 系 drive 的聲音，這是**使用者想要的
  voicing 改善**；每顆被重調的 drive 其 character 測試須更新並重新 pin（見 Phase 02）。
  已釋出 v0.1.0，但本專案目前單人使用、preset 皆在本機——voicing 改動以「ADR 交代
  + 測試重新 pin」處理即可，不需相容性包袱。
- **測試**：每個 WDF 核心要有（a）解方程殘差 `a = v + R·i(v)`、（b）對稱/非對稱、
  （c）飽和/有界（±1e6 狂推不 NaN）、（d）靜態轉移曲線對照離線高精度參考、
  （e）**白箱判別測試**（頻率相依削波——memoryless 不成立的行為）、（f）多 rate/
  block、silence→silence。
- **Bench**：每顆進 `cargo bench -p lh-dsp`，成本記入 `docs/benchmarks.md` 深水區段。
- **ADR/PRD**：架構級（Phase 02 tone stack 引擎、Phase 03 WDF 框架、Phase 05
  電晶體建模、Phase 06 ADAA、Phase 07 神經路徑）各開一支 ADR；每個 Phase 落地時
  對應一份正式 PRD。編號**於落地時依序取號**（PRD 022+、ADR 030+）——本目錄
  phase 檔內出現的 ADR 030/031/… 為**暫定代號**，實際號碼以落地順序為準（例如
  06 若先於 03 落地，其 ADR 就先取號）。
- **4× Oversample**：削波前沿用家族 `Oversampler4x`；若二極體轉角在 4× 下抗混疊
  不足，評估 8×（記入 ADR）。

## 8. 成功指標

- **目標 1**：換上真實 tone stack 後，同一顆 FMV 系 drive 在 noon 有可量測的中頻
  凹陷、三旋鈕互動可量測（轉 bass 改變 treble 響應）；耳朵上「像音箱不像 EQ」。
- **目標 2**：op-amp overdrive 家族（≥6 顆：TS/SD-1/ZenDrive/KoT/MXR/RAT）以白箱
  進 registry；fuzz/電晶體家族 ≥2 顆白箱（BigMuff、Rangemaster）＋ 既有 behavioral
  `fuzz-face` 保留/精修。各有 character pin 與 bench，全綠。
- **目標 3**：一份可跑的「netlist → 散射矩陣 → Rust 白箱」流程 + 驗證 harness；
  使用者能照食譜加一顆自己的 WDF 踏板（Phase 08 附範例）。
- **全程**：`cargo fmt/clippy/test` 全綠；`assert_no_alloc` 靜默；RTL/CPU 在預算內。

## 9. 非目標

- **不追 SPICE 位元級對拍**——目標是靜態曲線在容差內、動態行為可量測地優於
  memoryless、耳朵更像真踏板。
- **不做整顆踏板的每一級**（電源、旁通、緩衝）——只做決定音色的關鍵級（削波、
  tone stack、關鍵濾波），沿用 lion-heart 現有的 `shape()/post()/eq()` 分工。
- **不在 v1 動 engine/session 訊息集**——除非 Phase 07 神經路徑逼不得已（另議）。
- **不散布任何 GPL 或授權不明的模型權重/程式碼**（見 §4）。
- **本計畫不含 cab/IR、reverb、mod 等非 drive/tone 家族**——那些已完成或另有路線。

## 10. 詞彙表

- **WDF（Wave Digital Filter）**：把類比電路離散進「波域」（`a = v+Ri`、`b = v−Ri`）
  的方法；線性元件成 one-port，單一非線性放樹根，線性部分對它呈 Thévenin 等效。
- **R-Type adaptor**：處理無法用 series/parallel 化約的拓撲（如含 op-amp 回授的
  網路）的 N-port 適配器；核心是一個 N×N **散射矩陣** `b = S·a`，S 由各 port 阻抗
  算出（公式由 R-Solver 產生）。
- **Wright Omega**：解 `ω + ln(ω) = x` 的函數（Lambert W 的 e^x 版）；WDF 二極體
  方程 `a = v + R·i(v)` 可重排成一次 ω 求值 → 取代 Newton 迭代，零迭代、branch-free。
- **FMV / TMB tone stack**：Fender/Marshall/Vox 共用的被動三旋鈕（Treble-Middle-
  Bass）tone 網路；招牌中頻凹陷 + 強旋鈕互動。
- **ADAA（Antiderivative Anti-Aliasing）**：用波形整形函數的反導數做抗鋸齒，比純
  oversample 更有效抑制硬切產生的鋸齒。
- **NDK（Nodal DK method）**：另一種電路離散法（狀態空間），BYOD 的 Fuzz Face 用它。
- **R-Solver**：ChowDSP 的 Python 工具，從電路 netlist 自動產生 R-Type 散射矩陣。

## 11. v2 校訂摘要（2026-07-27）

初稿由較弱的模型產生；v2 對照 `/mnt/BYOD`、`/mnt/chowdsp_wdf` 原始碼與 lion-heart
現碼逐項查核後的修正，較大者如下（細節在各 phase 檔）：

1. **Phase 01 效能預期修正**：omega 只加速 **root 求解**（≥5×）；全踏板成本下限
   受 4× oversampler 地板（`ts9` ≈ 11.4 µs）限制，`screamer`/`sd1` 實際預期
   **~68–71 → ~20–30 µs（2–3×）**，非初稿的 10–15 µs。驗收標準已改為可達成的
   兩段式（root microbench + 全踏板上限）。
2. **Phase 01 數學定性修正**：eqn(39) 對**反並聯對（sinh）是高精度近似**（僅單向
   單二極體有精確閉式）；Newton 保留為 oracle。非對稱（`AsymDiode`）**先維持
   Newton**，omega 化為選配。
3. **Phase 02 元件值修正**：BYOD Bassman 的 R3=96k 是**它自己改過的**
   （stock 5F6-A 為 25k）——我們以原機 schematic 為準。另補：pot taper 映射、
   插入損 makeup、ngspice AC 掃描 golden fixtures。
4. **Phase 03 架構建議翻轉**：組法建議由「(b) 扁平陣列」改為 **(a) 靜態泛型 +
   擁有式子樹**——Rust 所有權下不需要 chowdsp 的 parent 指標與 defer-impedance
   機制（阻抗重算改為 block-rate 全樹一次 pass），(b) 的「codegen 較友善」不成立
   （codegen 產泛型樹一樣容易）。R-Solver bootstrap 從 Phase 08 提前為
   Phase 03 的前置工作項。
5. **Phase 04 新增移植陷阱**：BYOD ZenDrive 把二極體的阻抗參考掛在 `P1`（TS 掛
   `P3`）、波交換卻對 `P3`——幾乎可確定是 bug，其擬合參數（Vt≈0.0787，物理值
   3 倍）是**繞著這個 bug 擬合的**。我們按正確拓撲建模 + 自行再校準，不盲抄
   BYOD 擬合值。
6. **Phase 05 大幅簡化**：BYOD Big Muff **不是 WDF**——固定增益 CE 級 + 回授
   二極體的標量 Newton（與 `sd1` 同類機制，不需電晶體 root）；真正需要
   Ebers-Moll root 的只有 Rangemaster；Fuzz Face 的 NDK 碼由**私有工具**生成，
   正式改為「維持 behavioral、NDK 列未來研究」。BassFace/Flapjack/Warp/Blonde
   （BYOD 原創）移出主線。
7. **Phase 06 新增風險**：一階/二階 ADAA 引入 **~0.5/1 sample 群延遲**——與未
   延遲的 dry 路徑相加會產生高頻梳狀偏移，「character pin 不變」不保證成立；
   已補對齊/補償策略與測試要求。
8. **流程修辭修正**：全文「pre-v0.1」語境已過時（v0.1.0 已釋出）；PRD/ADR 編號
   改為落地時取號；06（ADAA）提前到 03 之前執行。
</content>
</invoke>
