# Phase 07 — 類神經 / 真空管 家族（可選 · 部分暫緩）

命中目標：#2 的剩餘一塊（Centaur/GuitarML/triode/tube）
依賴：一條**類神經推論路徑**（RTNeural 等效，或複用 `lh-nam` 的神經 seam）；
Phase 03（triode 走 WDF + 神經）
關聯 ADR：**新開 ADR 034（神經 drive 路徑）**；連結白皮書「triode stage」深水題、
ADR 027（跨平台）、`lh-nam`（既有 NAM seam）
來源與授權：⚠️ **模型權重是主要障礙**——BYOD 的 Centaur/GuitarML/RONN 權重多為
**GPL 或來源不明**，`lion-heart`(MIT/Apache) **不可散布**。程式碼技術可重寫，
**權重不可照搬**。

---

## 1. 背景與決策

BYOD 有一批**類神經**與**真空管**drive，與純電路白箱不同：

- **Centaur（Klon）**：`GainStageML`＝RTNeural 訓練的增益級 + WDF 削波 +
  summing amp。lion-heart 已有 memoryless `centaur`。
- **GuitarML Amp / RONN**：RTNeural（LSTM/隨機網路）建模整個 amp/drive。
- **Junior B**：`ModifiedRType` + `NeuralTriodeModel`——**類神經三極管**，正對
  白皮書「triode stage」深水題。
- **Tube Amp**：`TubeProc` 真空管級。

**兩個現實**：
1. lion-heart 的音色核心已經是**神經**（NAM captures via `lh-nam`/nam-rs）。所以
   「神經 drive」不是新概念——但 NAM 與 RTNeural 是**不同模型格式**，且 nam-rs 是
   amp capture，非 pedal-stage 小網路。
2. **權重授權**是硬約束：不能把 GPL/來源不明的權重塞進 MIT 專案散布。

**拍板**：本 Phase **可選、排最後、分兩條務實路**：

- **(A) 神經 pedal 級路徑**（若要做 Centaur ML/GuitarML）：需 (a) 一條輕量 RNN/MLP
  推論（可自寫，或評估 nam-rs 能否兼作 pedal 級網路），(b) **自行訓練或取得寬鬆
  授權的權重**（用 Phase 08 的 SPICE/re-amp 生資料集訓練——這反而是 lion-heart 的
  強項，因為已有 offline render pipeline）。
- **(B) 真空管/三極管白箱路徑**（triode，接白皮書深水題）：走 WDF triode root
  （Phase 03 框架 + 一個三極管非線性 root，如 Dempwolf-Zölzer 模型），**不需神經
  權重**——這條更乾淨、更符合「白箱」精神，建議優先於 (A)。

## 2. 規格

### 2.1 Triode WDF root（路線 B，建議優先）
- 在 `blocks::wdf` 加**三極管非線性 root**：以 Dempwolf-Zölzer（或 Koren）三極管
  電流式，omega/Newton 解，接 R-Type（BYOD JuniorB 的 `ModifiedRType` 對應）。
- 應用：一個**真空管前級 drive**（暖、偶次諧波、compression/sag），對接白皮書
  triode 深水題與 `power`（功率級）家族。Faceplate：Drive / Bias / Tone / Level。
- **不需外部權重**——純電路白箱，授權乾淨。

### 2.2 神經增益級路徑（路線 A，需資料集）
- 評估**推論引擎**：自寫小型 LSTM/GRU/MLP（RTNeural 是 MIT，可重寫其推論），或
  查 nam-rs 能否載入 pedal 級網路。RT 安全（權重 `prepare` 載入、熱路徑純前向、
  無配置）。
- **資料集自產**（關鍵，靠 Phase 08）：用 `lion-heart render`（ADR 023 offline
  re-amp）或 SPICE 生「輸入→輸出」對，訓練自己的 Centaur/透明 boost 網路 →
  **權重授權歸 lion-heart**，可散布。
- Centaur 可做**混合**：神經增益級 + WDF 二極體削波（BYOD 正是此結構）。

### 2.3 Tube Amp（可選）
- 若做，走 2.1 的 triode root 串成多級（前級 + 相位反相 + 功率級），或複用既有
  `power` 家族 + triode 前級。

## 3. 非目標

- **不散布任何 GPL/來源不明權重**——要嘛自訓、要嘛不做。
- **本 Phase 可整體暫緩**——不阻擋 Phase 01–06 的主線交付。「我要所有的 drive」
  的絕大多數由電路白箱（04/05）+ waveshaper（06）兌現；神經/真空管是「錦上添花 +
  白皮書深水題」。
- **不在此重做整個 NAM 路線**——nam-rs 是 amp 核心，這裡只評估其能否兼作 pedal 級。
- 不追即時訓練——訓練離線，載入權重即時。

## 4. 驗收標準

### 4.1 路線 B（triode，若做）
- `cargo test`：三極管 root 解方程殘差、有界（±1e6）、偶次諧波顯著（真空管特徵）、
  bias 掃描改變工作點、silence→silence、多 rate/block。
- `cargo bench`：triode root 成本；`assert_no_alloc` 乾淨。
- 耳朵：暖、偶次、動態 compression/sag、對接 power 級。

### 4.2 路線 A（神經，若做）
- 推論 RT 安全（`assert_no_alloc` 乾淨，權重 prepare 載入）。
- **權重可散布**（自訓或寬鬆授權）——授權審查通過才可合入。
- 對訓練目標（SPICE/re-amp 真值）的誤差在容差內；耳朵對真 Centaur A/B。

## 5. 產出清單

- （B）`blocks::wdf` triode root + `drive/triode.rs`（或 `tube_preamp.rs`）。
- （A，若做）神經推論模組 + 自訓權重 + 資料集產生腳本（依賴 Phase 08 render/SPICE）。
- **ADR 034**（神經 drive 路徑：推論引擎、權重授權策略、與 nam-rs 的關係、triode
  白箱 vs 神經的取捨）。
- **PRD 027**（正式版，若進主序列）。

## 6. 風險與備註

- **權重授權是紅線**——Centaur/GuitarML 的現成權重多不可用；務必自訓或找寬鬆來源。
- **triode 白箱（B）風險最低、最符合計畫精神、且推進白皮書深水題**——若本 Phase
  只做一件，做 triode。
- **神經（A）的真正價值在「用 lion-heart 自己的 render pipeline 生資料、訓練自有
  權重」**——這把 Phase 08 平台與神經路線接起來，是長線亮點但非短期必需。
- 本 Phase 標記為**可選**；優先級最低，待 01–06 穩定後再評估資源。
</content>
