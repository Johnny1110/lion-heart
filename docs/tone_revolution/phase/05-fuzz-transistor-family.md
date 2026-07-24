# Phase 05 — Fuzz / 電晶體 / booster 家族

命中目標：#2（fuzz 與電晶體類 drive）
依賴：Phase 03（WDF 框架；電晶體 root 可能需擴 Phase 01 的 root 求解）
關聯 ADR：若引入電晶體 WDF root 或 NDK → **新開 ADR 032（電晶體/fuzz 建模法）**
來源與授權：拓撲＋元件值＝事實；BYOD `drive/big_muff`、`drive/fuzz_machine`、
`drive/RangeBooster.cpp`、`drive/BassFace.cpp`（GPL，僅當教科書）。**不搬碼。**

---

## 1. 背景與決策

fuzz/電晶體類與 op-amp overdrive 不同——沒有 op-amp，靠**電晶體本身的非線性**
（Ebers-Moll 指數）。lion-heart 已有一顆 behavioral `fuzz-face`（tanh 上 + 硬夾下 +
bias offset + ratio gate），是很聰明的行為近似。本 Phase 把這族做「更白箱」，並補
Big Muff、Rangemaster、BassFace。

三種可用建模法（ADR 032 拍板）：

- **(a) WDF + 電晶體 root**：把 BJT 當非線性 root（Ebers-Moll 或簡化二極體式），
  用 omega/Newton 解。Big Muff 的級聯電晶體削波（`BigMuffClippingStage`）適合。
- **(b) NDK（Nodal DK method）**：BYOD Fuzz Face 用 `FuzzFaceNDK`——狀態空間法，
  對兩電晶體回授對（Fuzz Face 拓撲）比 WDF 好處理（WDF 對強耦合回授對較吃力）。
  成本：得引入一套 NDK 求解（矩陣 + 非線性迭代），是新機制。
- **(c) 保留/精修 behavioral**：lion-heart 的 `fuzz-face` 行為模型已抓到三大特徵
  （非對稱、gated decay、cleans up）。可只做小幅白箱化（bias/gate 由電路參數導出）。

**拍板**：Big Muff 走 (a) WDF 電晶體 root（最正統、最有價值）；Fuzz Face 評估
(b) NDK vs (c) 精修——若 NDK 成本過高，先 (c) 保留現有並記入 ADR「Fuzz Face NDF
暫緩」；Rangemaster/BassFace 走 (a) 或簡化行為。

## 2. 規格：踏板清單

### 2.1 Big Muff Pi — 新增 `big-muff`
BYOD `drive/big_muff`（`BigMuffClippingStage` + `BigMuffDrive`）、`drive/muff_clipper`。
- 招牌：**兩級級聯電晶體削波**（各級：電晶體增益 + 二極體對回授削波）+ 之間的
  RC 塑形 + **Big Muff tone stack**（中頻凹陷「反 wah」，見 Phase 02 `BigMuffTone`）。
- 建模：每級削波以 WDF 電晶體/二極體 root；級間 coupling cap 高通。厚、綿密、
  長 sustain 的 fuzz/distortion。
- Faceplate：Sustain(gain) / Tone / Volume。

### 2.2 Fuzz Face（germanium/silicon）— 升級既有 `fuzz-face` 或新增 `fuzz-face-wdf`
BYOD `drive/fuzz_machine`（`FuzzFaceNDK` + `fuzz_face_ndk_config.json`）。
- 招牌：兩電晶體回授對、**非對稱削波、gated/spluttery decay、cleans up from
  input**——lion-heart 現有 behavioral 版已抓到。
- 選項：(b) NDK 忠實版（需引入 NDK 求解）；或 (c) 保留現有、把 bias/gate 參數
  由電路值導出。ADR 記錄取捨。germanium vs silicon 可做 stepped 選項。

### 2.3 Rangemaster（Dallas，treble booster）— 新增 `rangemaster`
BYOD `sim/Rangemaster/rangemaster.py`、`drive/RangeBooster.cpp`。
- 單鍺電晶體 treble booster：**高通輸入**（招牌 treble boost）+ 電晶體軟削。
- 推 amp 前端的經典「Clapton/Beano」味。Faceplate：Boost / (Range) / Level。

### 2.4 Bass Face — 新增 `bass-face`
BYOD `drive/BassFace.cpp`：Fuzz Face 的低音化（低頻不 scoop、厚），適合貝斯或
厚身失真。可與 Fuzz Face 共用建模。

## 3. 非目標

- **不做整顆電源/旁通**——只做削波 + tone。
- **不追每顆電晶體的實測 β/Is**——用型號代表值，聽感校準後 pin。
- **Fuzz Face NDK 若成本過高則暫緩**——現有 behavioral 版可用，記入 ADR。
- **不抄 BYOD 的 NDK config/程式碼**（GPL）——NDK 若做，從公開文獻自行實作。

## 4. 驗收標準（每顆）

### 4.1 `cargo test`
- **有界/有限**（±1e6 狂推不 NaN；fuzz 自激/gate 要 bounded，RT 規則 7）。
- **character pin**：
  - `big-muff`：長 sustain（fade tail 比 TS 系顯著更持久，沿用 `monster5150`
    的 sustain 測試法）、tone 的中頻凹陷。
  - `fuzz-face`：強非對稱（h2 ≫ TS）、gated decay（held note tail/body ≪ TS）、
    cleans up（低輸入清乾淨）——沿用既有 `fuzz-face` 測試。
  - `rangemaster`：高通特性（低頻明顯衰減、treble boost）。
- 多 rate/block、silence→silence。

### 4.2 `cargo bench`
- 每顆進 bench；電晶體 root / NDK 成本記 `docs/benchmarks.md`。

### 4.3 `assert_no_alloc`
- select + 狂推 + gate 觸發全程零配置。

### 4.4 耳朵（使用者）
- `big-muff`：綿密牆式 fuzz、tone 掃過中頻凹陷。
- `fuzz-face`：splatty 非對稱、held note 的 velcro/gated 收尾、roll back 清乾淨。
- `rangemaster`：推 amp 的 treble boost 甜度。

## 5. 產出清單

- `crates/lh-dsp/src/drive/{big_muff,rangemaster,bass_face}.rs` + Fuzz Face 決策產物。
- 電晶體 root（若 (a)）或 NDK 求解（若 (b)）進 `blocks::wdf` / 新 `blocks::ndk`。
- registry 追加、livery、plugin id 展開（重跑 clap-validator）。
- **ADR 032**（電晶體/fuzz 建模法：WDF root vs NDK vs behavioral 的取捨）。
- **PRD 025**（正式版，若進主序列）。
- character/bench 測試。

## 6. 風險與備註

- **WDF 對 Fuzz Face 回授對吃力**：兩電晶體強耦合回授，WDF 需 R-Type 甚至多非線性
  root（難）；這正是 BYOD 用 NDK 的原因。務實：Big Muff（級聯、可 WDF）先做，
  Fuzz Face 視 NDK 成本再決定。
- **電晶體 root 的收斂**：Ebers-Moll 指數比二極體更 stiff；沿用 omega/damped
  Newton + 上界 + 狂推測試。
- Big Muff 是這族最有價值、最可行的一顆，建議先做。
</content>
