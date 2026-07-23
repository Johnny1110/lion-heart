# PRD 020: WDF Tube Screamer 削波級 — 白箱電路模擬（深水區第一題）

狀態：**已實作（ADR 028，2026-07-23）— 待使用者耳朵驗收**
日期：2026-07-23
里程碑：白皮書 §6 深水區研究線 · 第 1 題（WDF 白箱電路模擬）
關聯：白皮書 §6（深水區明列「WDF 白箱電路模擬，第一個題目：Tube Screamer
削波級」）、§5.3（音色核心）、ADR 003（drive 註冊表 + memoryless 波形整形
前例）、drive 家族（`Circuit` trait + `Oversampler4x` + append-only 前例）、
現有 `ts9` 行為模型（白箱模型的對照組）

> 實作落差（詳見 ADR 028）：(1) **建模範圍是 shunt clipper 而非回授拓撲**——
> 真實 TS 在 op-amp 回授迴路削波，v1 以「shunt clipper + op-amp 增益」忠實化約
> 可聽的二極體動態；理想 op-amp 迴路的 WDF（R-type adaptor）與非對稱二極體留
> v2。(2) **bench 成本高於估計**：PRD 估 ~20–40 µs，實測（x86 sandbox）**≈68 µs
> /block**，約 memoryless `ts9`（11 µs）的 6 倍、deadline 的 ~5.1%——Newton 的
> `f64 exp`（每過取樣 sample／聲道）是成本；接受為深水區白箱代價（仍在預算內，
> 且只在選用 screamer 時付出）。(3) **頻率相依判別測試移到 WDF core**（`clip()`
> 直接驅動，避開 720 Hz HP 的干擾）；全踏板對 `ts9` 的測試改斷言「兩者高頻聲音
> **可量測地不同**」（誠實主張——白箱不是換皮；WDF 較硬的二極體轉角其實比 ts9
> 的軟曲線＋51 pF lowpass 保留*更多*高頻 edge，方向與原假設相反）。

## 1. 背景與決策

目前 drive 家族的 11 顆踏板**全部是 memoryless 波形整形**——`tanh`、
`x/√(1+x²)`、多項式、cold-clipper 轉角——外掛一階濾波器塑形。這條路好聽、
便宜、可控，但它有一個先天限制：**削波是瞬時的、與頻率無關、與電抗元件毫無
互動**。真實削波電路的音色恰恰來自它沒有的東西：

- **RC 與二極體結電容的動態互動**——削波門檻會隨頻率與暫態移動（電容分流
  高頻，高頻等效削波點更高），這是「和弦破音」與「單音破音」不同質地的來源。
- **回授網路隨 drive 變化**——TS 的 51 pF 讓削波路徑在 drive 轉大時變暗。
- **對稱／非對稱由實際二極體決定**，而非一條手調曲線。

memoryless shaper 只能近似「平均」的削波曲線，抓不到這些動態。

**WDF（Wave Digital Filter）** 是把類比電路離散進「波域」的方法：每個元件是
一個 one-port（一個波阻抗 `R` + 入射波 `a` / 反射波 `b`，`a = v + R·i`、
`b = v − R·i`），用 series/parallel adaptor 連成一棵樹；**單一非線性放在樹根**，
線性部分對它呈現一個 Thévenin 等效（一個入射波 `a` + 一個 port 阻抗 `R`），
非線性只需在自己的 v–i 特性上解一個純量方程。這是把「**電路元件值 → 音色**」
的白箱路線——你調的是 1N4148 的 `Is`、電阻、電容，不是一條曲線的參數。白皮書
§6 明列此為深水區的第一題。

**拍板：** 交付兩件事——

1. **可重用的 `blocks::wdf` 框架**（深水區的地基，未來每顆白箱踏板複用）：
   one-port 原語（電阻、bilinear 電容、含內阻電壓源）、parallel/series
   adaptor（含反射自由 root port 的適配）、以及非線性 root 的求解介面。
2. **第一個應用**——把 **TS 削波級的「RC + 反相並聯二極體 clipper」建成 WDF**，
   作為一顆**新的 drive 踏板**（暫定 key `screamer`，名稱 "Screamer"）。
   `shape()` 在 4× oversample 下每個 sample 解一次 WDF；`post()` 沿用 TS 既有
   的線性聲道（720 Hz 輸入高通、mid-hump 乾濕相加、tone tilt、makeup）。

**新踏板、不取代 `ts9`。** `ts9` 是已校準、已測試（unity-loudness、mid-hump
pin）、已出貨的行為模型。白箱是**對照組**——研究這個 feature 的重點正是能讓
使用者 A/B「白箱 vs memoryless」、親耳判斷電路模擬值不值得那份 CPU。踏板是
**append-only**（`DRIVE_PEDALS` 追加），**無 preset schema bump**，plugin 自動
展開 `drive_screamer_*` 參數。

## 2. 規格

### 電路（v1 建模範圍：削波級）

以 op-amp 增益 `g`（drive 鈕→回授電阻）推進後，送入二極體削波網路：

```
Vin·g ──[ R ]──┬── C ──┐
               │       │
               ├─▶|─┐  │   (2× 1N4148 反相並聯)
               │◀|─┘  │
              GND     GND
```

即 **shunt RC 二極體 clipper**：串聯電阻 `R`、節點對地並聯 電容 `C` 與 反相並聯
的兩顆 1N4148。白箱輸入 = 實際元件值：二極體 `Is ≈ 2.52 nA`、`n ≈ 1.75`、
`Vt ≈ 25.85 mV`（1N4148 SPICE 代表值，可微調到味）；`R`、`C` 定 RC 轉角、放進
TS 的聲域（實作時定值、ADR 記錄，聽感校準後 pin）。

> 誠實範圍界定：真實 TS 削波級是**回授式** clipper（二極體在非反相 op-amp 的
> 回授迴路裡）。v1 以「shunt clipper + op-amp 增益」這個**忠實化約**捕捉可聽的
> 二極體削波動態（對稱軟削、RC 頻率相依門檻）；把理想 op-amp 迴路拓撲本身建成
> WDF（ideal-op-amp adaptor / R-type）是 v2 精修，不在本題。

### WDF 結構

三個分支（含內阻電壓源、電容、二極體）在節點**並聯** → 一個 **3-port parallel
adaptor**：

- **含內阻電壓源**（`Vin·g` 串 `R`）：port 阻抗 = `R`，反射波 `b = e`（適配後
  反射自由）。
- **電容**（bilinear/梯形離散）：port 阻抗 `R_C = T/(2C)`，反射波 `b[n] = a[n−1]`
  （狀態 = 前一入射波，一個單位延遲）。
- **二極體 root**：parallel adaptor 的第三 port 做成**反射自由**
  （`R_root = 1/(G_R + G_C)`，`G = 1/R`）——二極體「看見」`R` 與 `R_C` 的並聯。

adaptor 每 sample：先由電壓源波與電容狀態算出 root 的入射波 `a`，root 解出
反射波 `b`，`b` 再傳回更新電容狀態（`a_C[n] = ...` → 下一拍的 `b_C`）。

### 非線性 root 求解（RT 安全的核心）

反相並聯二極體對的 v–i：`i(v) = 2·Is·sinh(v / (n·Vt))`（對稱）。在波域要解

```
f(v) = v + R_root·i(v) − a = 0
```

用 **Newton–Raphson**：`f'(v) = 1 + (R_root·2·Is/(n·Vt))·cosh(v/(n·Vt))`。
`i(v)` 單調 → **保證收斂**、少數迭代即達容差。RT 安全要點：

- **固定迭代上限**（例如 ≤ 16）＋ 殘差門檻雙重停止；達上限也回一個有界值。
- **初值** = 前一 sample 的 `v`（訊號連續 → 通常 1–3 迭代收斂）。
- **overflow 夾制** `v/(n·Vt)`（`sinh`/`cosh` 引數上限）——狂推大訊號不產生
  `inf`/`NaN`。
- 解出 `v` 後 `b = 2v − a`。全程**零配置**、拓撲於 `prepare` 建好、迭代次數上界
  固定 → 通過 `assert_no_alloc`。

### Faceplate（沿用 TS 三鈕）

`drive`（op-amp 增益，映到回授電阻 0..10）/ `tone`（單極 dark↔bright tilt，
沿用 `ts9` 的 723 Hz）/ `level`（共用 `drive_law` level 法則）。

### 其他

- **Oversample 4×**（家族標準 `Oversampler4x`，削波前抗混疊）。若抗混疊測試
  顯示 4× 不足（二極體轉角比 tanh 硬），評估 8×，記入 ADR。
- **Livery**：新 signature 色，納入 distinct-livery pin。
- **plugin**：自動展開 `drive_screamer_*`——**pre-v0.1 additive id 新增**（無
  rename），重跑 clap-validator。

## 3. 非目標

- **不建整顆 TS**（輸入緩衝、電源、輸出級、旁通開關）——只做白皮書明列的
  「削波級」。
- **不取代 `ts9`**——行為模型保留作對照。
- **不做 R-type / 多非線性 WDF**（多顆不同型二極體、電晶體、理想 op-amp 迴路）
  ——v1 單一非線性 root。**非對稱**（上下不同二極體數，如 TS 的某些改機）與
  **回授式拓撲**留 v2。
- **不做自動微分 / 符號求解**——手寫 Newton。
- **不追 SPICE 位元級對拍**——目標是（a）靜態轉移曲線在容差內、（b）動態行為
  （頻率相依門檻）**可量測地**優於 memoryless、（c）聽感更像真踏板。

## 4. 驗收標準

1. **`cargo test`：**
   - **Newton 收斂**：全輸入範圍（含 ±狂推）迭代 ≤ 上限、殘差 < 容差；輸出
     有界、無 `NaN`/`inf`（RT 規則 7）。
   - **靜態轉移曲線**：DC 掃描對照離線高精度參考解（細步長或高倍過取樣的同一
     電路數值解）在容差內。
   - **削波門檻**：≈ 二極體順向壓降；**對稱**（反相並聯）→ 奇次諧波為主、偶次
     顯著偏低（與 `evva`/`jan-ray` 的強偶次對比）。
   - **頻率相依（白箱判別測試）**：高頻正弦的等效削波門檻 **高於** 低頻（RC
     分流高頻）——這對 memoryless `ts9` **不成立**，是白箱獨有行為，作為
     「模型確實在模電路」的證據。
   - **抗混疊**：高頻正弦輸入、4× OS 下混疊地板在門檻以下。
   - bypass 位元透明、多 rate/block（44.1/48/96 kHz，block 32–1024）、
     silence→silence（reset 後）。
2. **`cargo bench`：** `screamer`（含 Newton + 4× OS）每 64-frame stereo block
   成本——**預期高於 memoryless 踏板**（每 sample 數次 Newton 迭代）。設定期望值
   （估 ~20–40 µs/block，仍 < 3% deadline），記入 `docs/benchmarks.md` 的
   深水區段落，並註明「研究線的白箱成本，可接受」。
3. **`assert_no_alloc`：** select 到 `screamer` 並狂推全程無配置（Newton 無 heap、
   迭代上限固定）。
4. **耳朵驗收（使用者）：** 同一段 riff／和弦，`screamer` 對 `ts9` A/B——白箱在
   暫態與和弦下的「觸感」、頻率相依削波（低把位厚、高把位不糊）、drive/tone
   交互是否更像真 Tube Screamer；roll back 吉他音量的破音收乾是否更自然；
   不炸、不混疊。
