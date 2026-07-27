# Phase 05 — Fuzz / 電晶體 / booster 家族

命中目標：#2（fuzz 與電晶體類 drive）
依賴：Phase 01（Newton/omega root 機制沿用）；**不依賴 Phase 03 R-Type**
（v2 查核：本族在 BYOD 全是標量 Newton/NDK，無 op-amp R-Type——可與 Phase 04
平行做）
關聯 ADR：**新開 ADR（暫定 032，電晶體/fuzz 建模法）**
來源與授權：拓撲＋元件值＝事實；BYOD `drive/big_muff`、`drive/fuzz_machine`、
`drive/RangeBooster.cpp`（GPL，僅當教科書）。**不搬碼。**

---

## 1. 背景與決策

fuzz/電晶體類與 op-amp overdrive 不同——沒有 op-amp，靠**電晶體本身的非線性**。
lion-heart 已有一顆 behavioral `fuzz-face`（tanh 上 + 硬夾下 + bias offset +
ratio gate），是很聰明的行為近似。本 Phase 把這族做「更白箱」，並補 Big Muff、
Rangemaster。

**v2 查核修正**：初稿把這族統稱「WDF 電晶體 root」——讀 BYOD 原始碼後不成立，
實際是三種**互不相同**的機制，難度差很多：

- **(a) 固定增益級 + 回授二極體 root（Big Muff 的實法）**：BYOD
  `BigMuffClippingStage` 把 BJT 共射級**線性化**為固定增益 `A = −R_c/R_e ≈
  −66.7`，非線性只有回授路徑上的反並聯二極體（`2·Is·sinh`）＋ `C12`，逐 sample
  對輸出節點做**標量 damped Newton**。這與 lion-heart `sd1` 的「回授二極體 root」
  是同一類問題——**不需要任何新求解機制**，難度大降。
- **(b) 完整 Ebers-Moll 標量 Newton（Rangemaster 的實法）**：BYOD
  `RangeBooster` 對單顆 BJT 解完整 Ebers-Moll（`v_be`/`v_bc` 指數、βF/βR），
  仍是標量迭代、warm-start、固定上界——這才是真正的「電晶體 root」，是
  ADR 032 的核心新件。
- **(c) NDK 狀態空間（BYOD Fuzz Face 的實法）**：兩電晶體強耦合回授對，WDF/
  標量法都吃力。⚠️ BYOD 的 `FuzzFaceNDK` 由**私有工具**（`ChowDSP/Research/
  NDK-Framework`，未公開）生成——沒有現成可港的公開產生器，從文獻（Holters/
  Zölzer DK 法）自建是一整套框架的工作量。
- **(d) 保留/精修 behavioral**：現有 `fuzz-face` 已抓到三大特徵（非對稱、gated
  decay、cleans up）。

**拍板（v2）**：Big Muff 走 (a)（機制既有，最有價值且最可行）；Rangemaster 走
(b)（引入 Ebers-Moll root，範圍小、單電晶體）；**Fuzz Face 走 (d) 維持
behavioral**（可小幅白箱化：bias/gate 常數由電路值導出）——NDK 正式列**未來
研究線**（潛在深水區 #4），不入本計畫範圍；ADR 032 記錄這整組取捨。

## 2. 規格：踏板清單

### 2.1 Big Muff Pi — 新增 `big-muff`
BYOD `drive/big_muff`（`BigMuffClippingStage` + `BigMuffDrive`）、`drive/muff_clipper`。
- 招牌：**兩級級聯削波**＋級間 RC 塑形＋ **Big Muff tone stack**（中頻凹陷
  「反 wah」，直接用 Phase 02 的 `BigMuffTone`）＋ sustain（輸入增益）。
- 每級的電路事實（自 BYOD 佐證，對照 Big Muff schematic）：輸入濾波
  （C5=100nF/R19=10k/R20=100k，bias≈0.7V）→ 共射級固定增益 `A≈−66.7` →
  回授路徑 反並聯二極體（1N4148 系 `2·Is·sinh`）‖ `R17=470k` ‖ `C12=470pF`。
- 建模（v2 修正）：每級對輸出節點做**標量 damped Newton**（f64、warm-start、
  固定上界——`blocks::wdf` 的 Newton 慣例直接搬），`C12` 一階雙線性狀態。
  **不需電晶體 root、不需 R-Type**。兩級級聯 + coupling cap 高通。厚、綿密、
  長 sustain 的 fuzz/distortion。
- Faceplate：Sustain(gain) / Tone / Volume。

### 2.2 Fuzz Face — 維持 behavioral（精修選配），**不做 NDK**
BYOD `drive/fuzz_machine`（`FuzzFaceNDK` + `fuzz_face_ndk_config.json`）。
- 招牌：兩電晶體回授對、**非對稱削波、gated/spluttery decay、cleans up from
  input**——lion-heart 現有 behavioral 版已抓到。
- v2 拍板：NDK 碼由私有工具生成、自建 NDK 是框架級工作量（見 §1(c)）——
  **本計畫不做**；現有 `fuzz-face` 保留，選配小幅精修（germanium/silicon
  stepped 選項、bias/gate 由電路值導出）。NDK 列未來研究線，ADR 032 記錄。

### 2.3 Rangemaster（Dallas，treble booster）— 新增 `rangemaster`
BYOD `sim/Rangemaster/rangemaster.py`、`drive/RangeBooster.cpp`。
- 單鍺電晶體 treble booster：小輸入耦合電容形成**高通輸入**（招牌 treble
  boost）+ 電晶體軟削與 bias 不對稱。
- 電路事實（自 BYOD 佐證）：V+=9V、R1=470k/R2=68k 分壓 bias、RV=10k、
  emitter C3=47µF/R3=3.9k；Ebers-Moll 參數起點 `Is=10fA, βF=200, βR=2`
  （鍺管實機參數再校準——OC44 的 Is 量級更大，聽感定案）。
- 建模：**Ebers-Moll 標量 Newton root**（§1(b)，ADR 032 的新件；沿用 damped/
  warm-start/上界慣例，狂推測試守 RT 規則 7）。
- 推 amp 前端的經典「Clapton/Beano」味。Faceplate：Boost / (Range) / Level。

> **Bass Face 移出主線**（v2）：BYOD 原創變體、非業界名機（見 overview §5a
> 排除原則）。真想要時是 `fuzz-face` behavioral 的低頻 voicing 變體，一天工作量。

## 3. 非目標

- **不做整顆電源/旁通**——只做削波 + tone。
- **不追每顆電晶體的實測 β/Is**——用型號代表值，聽感校準後 pin。
- **不做 NDK**（v2 定案）——Fuzz Face 維持 behavioral；NDK 是未來研究線
  （自建須從公開文獻實作，且**不抄 BYOD 的 NDK config/程式碼**（GPL））。
- **不做 BassFace**（BYOD 原創，移出主線）。

## 4. 驗收標準（每顆）

### 4.1 `cargo test`
- **解方程殘差**：`big-muff` 每級的節點方程、`rangemaster` 的 Ebers-Moll 殘差
  代回容差內（同 `wdf.rs` 的殘差測試慣例）。
- **有界/有限**（±1e6 狂推不 NaN；fuzz 自激/gate 要 bounded，RT 規則 7）。
- **白箱判別**：`big-muff` 的 C12 使削波頻率相依（同 screamer 的判別法）。
- **character pin**：
  - `big-muff`：長 sustain（fade tail 比 TS 系顯著更持久，沿用 `monster5150`
    的 sustain 測試法）、tone 的中頻凹陷。
  - `rangemaster`：高通特性（低頻明顯衰減、treble boost）、bias 不對稱
    （偶次諧波存在）。
  - `fuzz-face`（僅若做精修）：既有三特徵測試維持全綠。
- 多 rate/block、silence→silence。

### 4.2 `cargo bench`
- 每顆進 bench；Newton 級成本（big-muff 兩級、rangemaster Ebers-Moll）記
  `docs/benchmarks.md`（預期與 `sd1` 同量級——同是標量 Newton）。

### 4.3 `assert_no_alloc`
- select + 狂推 + gate 觸發全程零配置。

### 4.4 耳朵（使用者）
- `big-muff`：綿密牆式 fuzz、tone 掃過中頻凹陷。
- `fuzz-face`：splatty 非對稱、held note 的 velcro/gated 收尾、roll back 清乾淨。
- `rangemaster`：推 amp 的 treble boost 甜度。

## 5. 產出清單

- `crates/lh-dsp/src/drive/{big_muff,rangemaster}.rs`（+ 選配的 fuzz-face 精修）。
- `blocks::wdf`（或 `blocks::transistor`）：muff 級標量 Newton helper、
  Ebers-Moll root。
- registry 追加、livery、plugin id 展開（重跑 clap-validator）。
- **ADR**（暫定 032）：電晶體/fuzz 建模法（固定增益+回授二極體 vs Ebers-Moll
  root vs NDK vs behavioral 的四方取捨；NDK 列未來研究）。
- **PRD**：落地時於主序列取號。
- character/bench 測試。

## 6. 風險與備註

- **Ebers-Moll 的收斂**：指數比二極體 stiff（兩個耦合指數項）；沿用 damped
  Newton + warm-start + 固定上界 + 狂推測試。BYOD `RangeBooster` 用了「先解
  `v_be`、內迭代 `v_bc`」的巢狀結構——移植時保持迭代上界為 const、debug assert
  殘差。
- **Big Muff 版本差**：Muff 歷代電路值變動大（Triangle/Ram's Head/Sovtek…）；
  先做一版代表值（BYOD 佐證的），版本變體留給可選 stepped（如日後想要）。
- Big Muff 是這族最有價值、最可行的一顆（機制與 `sd1` 同類），建議先做。
- **本 Phase 不再依賴 Phase 03**（v2）——可提前與 Phase 04 平行，或按人力排序。
</content>
