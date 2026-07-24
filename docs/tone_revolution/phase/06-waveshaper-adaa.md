# Phase 06 — Memoryless Waveshaper bank + ADAA 抗鋸齒

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
   （Antiderivative Anti-Aliasing，反導數抗鋸齒）+ 可變 OS（最高 16×）**。ADAA
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

### 2.3 既有 drive 抗鋸齒改造（品質，voicing 不變）

把 `ts9`/`bd2`/`classic`/`overdrive`/`red-charlie`/`monster5150`/`angry-charlie*`
等 memoryless 的硬切/多項式級接上 ADAA。**目標：同一 voicing、更乾淨的高把位**
（character pin 不動，另加抗鋸齒地板測試）。這是對「drive 不滿意」的通用去毛躁，
不必逐顆改電路。

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
- **既有 drive 改造**：character pin **不變**（voicing 保持）+ 新增抗鋸齒地板測試通過。
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
- **ADR 033**（ADAA 抗鋸齒策略；是否家族級預設）。
- **PRD 026**（正式版，若進主序列）。

## 6. 風險與備註

- **ADAA 在低增益/小訊號**：`x[n]≈x[n−1]` 頻繁，退化分支要穩且不引入 DC。
- **二階 ADAA 的延遲/暫態**：二階用到前兩 sample，暫態響應略軟；硬切用二階、
  軟飽和用一階即可。
- **這是最能獨立、且對「所有既有 drive」通用受益的一步**——可早做，立刻去毛躁。
</content>
