# Phase 06 — Memoryless Waveshaper bank + ADAA 抗鋸齒

> **已實作（2026-07-27）— PRD 024 / ADR 031。** 計畫與實際交付的差異記在這裡，
> 計畫本身不改：
>
> 1. **§2.1 二階 ADAA 改為自行從定義推導。** 計畫引 Parker 等人的發表形式；實作
>    對三角核直接分部積分，得到兩半對稱、**各自獨立退化**的形式，退化分支是單次
>    `f` 求值而非巢狀 case。正確性以「線性 `f` 還原 `(x₀+4x₁+x₂)/6`」＋「對照定義
>    積分的數值求解」雙重 pin，不靠推導背書。
> 2. **§2.3 的 dry-sum 對齊：三套補償方案一套都不需要。** 計畫把它列為「最容易踩
>    的坑」，但那是假設 ADAA 在基頻。實際 ADAA 跑在 **4× 率**上，折算基頻只有
>    0.125 sample——最壞漣漪 10 kHz 0.09 dB、16 kHz 0.22 dB。**既有 character pin
>    一條都沒改、全部通過**，包含 level-norm。（若過取樣倍率下修，這個結論要重驗，
>    測試裡有註記。）
> 3. **改造範圍擴大到全部 12 顆 memoryless 踏板**（計畫點名 8 顆）。`centaur`、
>    `jan-ray`、`fuzz-face` 一併做了——量測顯示它們同樣受益 23–27 dB。
>    `screamer`/`sd1` 是 WDF，沒有顯式曲線可談，不接。
> 4. **§2.2 shape bank 交付 12 條**（計畫列了約 15 條的清單）：Soft/Hard/Asym/
>    Diode/Sine/Fold/Digital/Cheby2–5/Fuzz。`zamsat`、`westfold`、trig 組未做——
>    bank 是 append-only 的資料，補起來不動引擎。
>
> 完整數字（每顆的地板前後、ADAA 的隔離成本）見 PRD 024 §3 與 `docs/benchmarks.md`。


命中目標：#2（memoryless 創意波形整形）＋ 全家族**品質**（去毛躁）
依賴：無（memoryless，不依賴 WDF 框架；可與任一 Phase 平行）
關聯 ADR：若 ADAA 成為家族級抗鋸齒策略 → **新開 ADR 033（ADAA 抗鋸齒）**
來源與授權：ADAA 技術＝Parker/Esqueda/Bilbao 等公開文獻；BYOD/Surge waveshaper
（**GPL**）僅當「有哪些形狀」的參考。**依數學自行重寫，不搬 GPL 碼。**

---

## 1. 背景與決策

兩個獨立但相關的主題：

1. **抗鋸齒（品質，全家族受益）。** lion-heart 的 memoryless drive 用固定 4× OS +
   硬切。硬切/多項式 shaper 產生的高階諧波遠超 Nyquist，4× OS 未必壓得乾淨——
   聽感上就是高把位「毛躁、沙沙、數位感」。這**可能正是使用者「drive 不滿意」的
   一部分**（與 tone stack 並列）。BYOD 的 Surge waveshaper 用 **ADAA
   （Antiderivative Anti-Aliasing，反導數抗鋸齒）**（已查核 `SurgeWaveshapers.cpp`
   的 ADAA kernel；另有 BYOD 全域可變 oversampling 疊加）。ADAA
   對硬轉角特別有效：用波形函數的**一階/二階反導數**做差分，等效在每個 sample
   內做解析積分平均，把鋸齒壓得比純 OS 低很多。

2. **創意波形整形 bank（新踏板）。** Surge 有數十種形狀（soft/hard/asym/sine/
   digital/wavefolder/chebyshev/fuzz/trig…），是 lion-heart 沒有的「數位/合成器味」
   失真調色盤。

**拍板**：交付 (a) 一個可重用的 **ADAA 波形整形基礎設施**（`blocks::waveshaper`），
(b) 用它把既有 memoryless drive 的硬切級**改造抗鋸齒**（去毛躁，voicing 不變），
(c) 一顆新的 **`waveshaper` 踏板**承載 shape bank。

## 2. 規格

### 2.1 ADAA 基礎設施（`crates/lh-dsp/src/blocks/waveshaper.rs`）

- **一階 ADAA**：給整形函數 `f(x)` 與其反導數 `F1(x)`，輸出
  `y[n] = (F1(x[n]) − F1(x[n−1])) / (x[n] − x[n−1])`；`|x[n]−x[n−1]|` 極小時退回
  `f((x[n]+x[n−1])/2)`（避免除零，數值穩定）。狀態＝上一個 `x` 與 `F1(x)`。
- **二階 ADAA**（可選，更乾淨）：需 `F2`（二次反導數）與前兩個 sample，照 Parker
  et al. 公式。硬轉角建議二階。
- **與 4× OS 疊加**：ADAA 不取代 OS，是**加在 OS 之後**（BYOD 也是 ADAA + OS）；
  兩者疊加對硬切最有效。是否降 OS 倍率換 ADAA（省 CPU）由 bench 決定。
- **群延遲（v2 新增，重要）**：一階 ADAA 天生帶 **~0.5 sample**（OS rate）延遲、
  二階 ~1 sample——「相鄰兩點的解析平均」的代價。單獨一條非線性路徑無感；
  **與未延遲的 dry 路徑相加就有感**（高頻梳狀/相位偏移）——這正是 `ts9` 的
  `x + clipped` 結構。對策見 §2.3。
- **RT 安全**：純函數 + 少量狀態，無配置；denormal flush；`x` 相等的退化分支
  branchless 或有界。

### 2.2 Shape bank（`waveshaper` 踏板）

以 Rust 重寫一組形狀（**依數學，不抄 Surge GPL 碼**），每個附其反導數供 ADAA：

- 飽和：`soft`(tanh)、`hard`(clamp)、`asym`（非對稱）、`zamsat`。
- 效果：`sine`（sin 摺疊）、`digital`（量化階梯）。
- Wavefolder：`singlefold`/`dualfold`/`westfold`（West Coast 摺疊）。
- Chebyshev：`cheby2..5`（純偶/奇次諧波生成）。
- Fuzz：`fuzz`/`fuzzheavy`/`fuzzctr`。
- （選）Trig/加法諧波組。

Faceplate：Drive / Shape（stepped，選形狀）/ Level。tone 可選加一個 post LP。

### 2.3 既有 drive 抗鋸齒改造（品質，voicing 幾乎不變）

把 `ts9`/`bd2`/`classic`/`overdrive`/`red-charlie`/`monster5150`/`angry-charlie*`
等 memoryless 的硬切/多項式級接上 ADAA。**目標：同一 voicing、更乾淨的高把位**。
這是對「drive 不滿意」的通用去毛躁，不必逐顆改電路。

**v2 修正——「character pin 一字不動」不保證成立**，因為 ADAA 的 ~0.5 sample
群延遲會讓「dry + wet 相加」結構（如 `ts9` 的 `x + clipped`）在高頻產生梳狀
偏移。逐顆處理，三選一並記錄：
1. **dry 路徑補等量半 sample 延遲**（OS rate 上一個一階 allpass/線性內插——
   便宜），相加對齊 → voicing 真正不變；
2. 無 dry-sum 的踏板（純串接 shaper）直接上，pin 預期不動；
3. 若補償不划算且頻譜差可聞測（>0.5 dB @ >5 kHz），重新 pin 並在 ADR 註記
   「ADAA 改造的微幅 voicing 變動」。
測試面：character pin 對照時把 dry/wet 對齊納入 harness（見 §4.1）。

## 3. 非目標

- **不搬 Surge/BYOD 的 waveshaper 程式碼**（GPL）——依公開數學重寫。
- **不改 voicing**（2.3）——抗鋸齒是「同曲線、少鋸齒」，character 不變。
- **不做 WDF**——本 Phase 純 memoryless。
- 不追「完全零混疊」——ADAA + OS 把地板壓到門檻下即可。

## 4. 驗收標準

### 4.1 `cargo test`
- **ADAA 正確性**：對 `hard`/`soft`，ADAA 輸出對照高倍 OS 參考（如 32×）在容差內；
  `x[n]≈x[n−1]` 退化分支不 NaN、連續。
- **抗鋸齒地板**：高頻正弦（如 5–10 kHz）輸入，ADAA+4×OS 的混疊分量**顯著低於**
  純 4×OS（量測非諧波地板，dB 差記錄）。
- **shape bank**：每形狀有界、silence→silence；Chebyshev N 階生第 N 諧波
  （Goertzel 驗證）；wavefolder 摺疊次數隨 drive 增加。
- **既有 drive 改造**：character pin 依 §2.3 對策逐顆處理——補償者 pin 不變、
  重 pin 者頻譜差在記錄容差內；全數新增抗鋸齒地板測試通過。
- 多 rate/block。

### 4.2 `cargo bench`
- `waveshaper_adaa1` / `adaa2` 對照無 ADAA；每形狀成本；既有 drive 改造前後成本差。
  記 `docs/benchmarks.md`。

### 4.3 `assert_no_alloc`
- `waveshaper` 踏板 + 改造後既有 drive，select + 狂推 + 切形狀零配置。

### 4.4 耳朵（使用者）
- 高把位單音/和弦，既有 drive 改 ADAA 前後 A/B——高頻「毛躁/沙沙」是否明顯減少、
  voicing 是否維持。
- `waveshaper` 踏板掃形狀：wavefolder 的 West Coast 味、Chebyshev 的純諧波、
  digital 的 lo-fi。

## 5. 產出清單

- `crates/lh-dsp/src/blocks/waveshaper.rs`：ADAA（一/二階）+ shape 函式庫（含反導數）。
- `crates/lh-dsp/src/drive/waveshaper.rs`：新踏板。
- 既有 memoryless drive 的硬切級接 ADAA。
- registry 追加、livery、plugin id；character 保持 + 抗鋸齒地板測試；bench。
- **ADR**（暫定 033，ADAA 抗鋸齒策略；是否家族級預設；dry-sum 對齊策略）。
- **PRD**：落地時於主序列取號。

## 6. 風險與備註

- **ADAA 在低增益/小訊號**：`x[n]≈x[n−1]` 頻繁，退化分支要穩且不引入 DC。
- **二階 ADAA 的延遲/暫態**：二階用到前兩 sample（~1 sample 延遲），暫態響應
  略軟；硬切用二階、軟飽和用一階即可。dry-sum 交互見 §2.1/§2.3——這是本 Phase
  最容易踩的坑，測試 harness 先行。
- **這是最能獨立、且對「所有既有 drive」通用受益的一步**——v2 已把它提前到
  Phase 03 之前（見 overview §6）。
</content>
