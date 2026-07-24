# PRD 021: 回授拓撲 op-amp 過載 + 非對稱削波 — WDF 白箱電路（深水區第二題）

狀態：**已實作（ADR 029，2026-07-24）— 待使用者耳朵驗收**
日期：2026-07-24
里程碑：白皮書 §6 深水區研究線 · 第 2 題（延續 WDF 白箱，回授拓撲 + 非對稱二極體）
關聯：PRD 020 / ADR 028（深水區第一題：WDF TS shunt clipper——本題正是它明列的
**v2**）、白皮書 §6（WDF 白箱電路模擬）、drive 家族（`Circuit` trait +
`Oversampler4x` + append-only 前例）、`blocks::wdf` substrate（`Capacitor` /
`DiodePair` / `parallel_root`）、現有 `screamer`（v1 白箱，本題的對照組之一）

## 1. 背景與決策

PRD 020 交付了兩件事：可重用的 **`blocks::wdf`** substrate，以及第一顆白箱踏板
`screamer`（TS 削波級的 shunt RC-二極體 clipper）。但它在「非目標」明確把兩件事
留給 v2：

1. **回授式拓撲**——真實 Tube Screamer / SD-1 的二極體是在 **op-amp 回授迴路**
   裡削波，不是對地 shunt。v1 用「shunt clipper + op-amp 增益」忠實化約，把
   mid-hump 用一條 720 Hz 輸入高通 + 乾訊號相加手工湊出來。
2. **非對稱二極體**——v1 只做對稱反相並聯（`i = 2·Is·sinh`），奇次諧波為主。
   很多 TS 改機（以及 SD-1）用「上下不同顆數」的二極體，產生偶次諧波。

**本題一次交付這兩件事，載體 = Boss SD-1「Super OverDrive」。** SD-1 跟 TS 同一個
op-amp 回授過載拓撲，但它的**簽名特徵正是非對稱削波**——回授迴路裡放 **2 顆串聯
＋ 1 顆反向** 的矽二極體（TS 是 1＋1 對稱）。所以做 SD-1：

- 剛好把 v2 的兩個 substrate 缺口都練到（回授拓撲 + 非對稱 root）；
- 得到一顆**真實、有名、聲音明顯不同**的踏板，不是「更準一點的 screamer」——
  SD-1 的偶次諧波、比 TS「更開」的音色，是白箱新拓撲直接長出來的；
- 維持家族的品牌建模慣例（`ts9`=Ibanez、`bd2`=Boss Blues Driver、`centaur`=Klon、
  `jan-ray`=Vemuram……），SD-1 用 key `sd1`（與 `bd2` 同慣例）。

**拍板：交付三件事——**

1. **substrate 補兩塊**（`blocks::wdf`，未來每顆回授式白箱踏板複用）：
   - **`AsymDiode`**：`m` 顆順向串聯 / `k` 顆逆向串聯的反並聯二極體 root，
     `i(v) = Is·(exp(v/(m·nVt)) − exp(−v/(k·nVt)))`（`m=k=1` 時退化成
     `DiodePair` 的 `2·Is·sinh`）。同樣是 warm-start damped Newton、`f64`、
     固定迭代上限、exp 夾制——有界、finite、零配置。
   - **`parallel_root_with_source`**：帶電流注入的並聯 root
     （`a = (Σ Gₖaₖ + I)/Σ Gₖ`、`R = 1/Σ Gₖ`）——回授迴路的 op-amp 強迫電流就靠
     這個灌進節點。`parallel_root` 不動（`screamer` 位元不變）。

2. **回授拓撲用 ideal-op-amp 解析化約（virtual short）**——不建通用 R-type nullor
   散射矩陣（那是更後面的事），而是用理想 op-amp 的虛短路把單顆 op-amp 迴路
   **正確化約**成一條可一次求解的直線：
   - op-amp 逼 `V(−) = V(+) = Vin`；增益腳（`R_gain` 串 `C_g` 對地）在 `Vin` 下
     流出電流 `I_g`（線性、只跟 `Vin` 與 `C_g` 狀態有關）；
   - KCL：`I_g` 被強迫灌進回授網路 `[R_f ‖ C_f ‖ 非對稱二極體]`，解出跨壓 `V_fb`
     （非線性 WDF root）；
   - `Vout = Vin + V_fb`。
   結果：**乾訊號結構性穿透**（最小增益恆為 1 → TS 永遠不會全 fuzz，這是 v1 用
   手工乾濕相加假裝的）；**mid-hump 由增益腳 RC 天生湧現**（`C_g` 讓低頻增益掉向
   1）——不再需要 v1 的 720 Hz 手調高通；`C_f`（51 pF）跨在二極體上，天生把高頻
   削波磨圓（真 TS 的「不刺」頂端）。

3. **新踏板 `sd1`「Super Drive」**——append-only（`DRIVE_PEDALS` 12→13、
   `MODEL_COUNT` 12→13）、**無 preset schema bump**、plugin 自動展開
   `drive_sd1_*`（pre-v0.1 additive id 新增，重跑 clap-validator）。
   **`screamer`、`ts9` 一律不動** → 使用者能三顆 A/B：
   `ts9`（memoryless 對稱曲線）／`screamer`（WDF shunt 對稱）／`sd1`（WDF 回授
   非對稱）——這正是深水區研究線的重點：親耳判斷拓撲與非對稱值不值得那份 CPU。

## 2. 規格

### 電路（SD-1 削波放大級）

非反相 op-amp 過載級，二極體在回授：

```
              ┌───[ R_f 51k ]───┐
              │                  │
   Vin ──[+]  ├──[ D1 ▶|─D2 ▶| ]┤   (回授二極體：2 顆順向串聯…)
        │ op  ├──[ D3 |◀ ]──────┤   (…並上 1 顆反向 → 非對稱)
        └─[−]─├──[ C_f 51pF ]───┤
          │   │                  │
          │   └──────────────────┴──── Vout
          │
          └──[ R_4 4.7k ]──[ Drive 0..100k ]──[ C_g 0.047µF ]── GND
```

代表值（實作時定值、聽感校準後 pin，記入 ADR）：
- **回授**：`R_f = 51 kΩ`、`C_f = 51 pF`（高頻削波平滑）、非對稱二極體
  `m_fwd = 2` / `m_rev = 1`（1N4148 `Is 2.52 nA / n 1.75 / Vt 25.85 mV`）。
- **增益腳**：`R_4 = 4.7 kΩ` 固定 + **Drive 電位器 100 kΩ**（key `sd1` 用 SD-1 的
  100 k，比 TS 的 500 k 緊）+ `C_g = 0.047 µF`。
- 小訊號中高頻增益 `≈ 1 + R_f/(R_4 + R_drive)`：Drive 10 → `≈ 11.9`，Drive 0
  → `≈ 1.5`。`C_g` 的轉角隨 Drive 在 ~32 Hz（低）到 ~720 Hz（高）間移動——
  **mid-hump 隨 drive 深淺自動出現**。

### WDF 結構與每（過取樣）sample 求解

1. **增益腳** = 串聯 `R_gain(=R_4+R_drive)` + `C_g` 對地，被 `Vin` 電壓驅動。用
   雙線性（trapezoidal）離散直接解出電流 `I_g` 並更新 `C_g` 狀態（線性、精確、
   RT 安全）：
   - `V_cg[n] = (Vin + R_gain·G2·V_cg[n−1] + R_gain·I_g[n−1]) / (1 + R_gain·G2)`，
     `G2 = 2·C_g·fs_os`；
   - `I_g[n] = G2·(V_cg[n] − V_cg[n−1]) − I_g[n−1]`。
   （這條線性腳留在踏板檔——電路專屬 voicing，像 screamer 的 `R_SERIES`；WDF
   substrate 只承擔非線性節點。）
2. **回授網路** = `[R_f ‖ C_f ‖ AsymDiode]`，被電流源 `I_g` 灌驅：
   - `a_root = parallel_root_with_source([(G_f, 0), (G_cf, a_cf)], I_g)` 的入射波、
     `r_root` 為 root 阻抗；
   - `(V_fb, _) = asym_diode.solve(a_root, r_root)`；`c_f.set_incident(2·V_fb − a_cf)`。
3. `Vout = Vin + V_fb`。

全程零配置、拓撲於 `prepare` 建好、Newton 迭代上限固定 → 過 `assert_no_alloc`。
過取樣 **4×**（家族標準 `Oversampler4x`）；`C_f`/`C_g` 於過取樣率離散。

### Faceplate（SD-1 三鈕）

`drive`（增益腳 `R_drive`，0..10 audio taper）/ `tone`（單極 dark↔bright tilt，
沿用 ts9/screamer 的 723 Hz）/ `level`（共用 `drive_law` level 法則）。
`post()`：tone tilt + makeup（校準到 drive 5 / tone 5 / level 6 近 unity）+ DC block。

### 其他

- **Livery**：SD-1 招牌**芥末黃**新 signature 色，納入 distinct-livery pin。
- **plugin**：自動展開 `drive_sd1_*`——pre-v0.1 additive id 新增（無 rename），
  重跑 clap-validator。
- **bench**：`effects.rs` 已迴圈家族 → 自動新增 `drive_sd1_4x_oversampled`；
  預期與 screamer 同量級（含 Newton + 4× OS），記入 `docs/benchmarks.md` 深水區段。

## 3. 非目標

- **不做通用 Werner R-type nullor 散射矩陣**——單顆 op-amp 迴路用解析 virtual-short
  化約即正確且更省；多 op-amp / 多非線性的通用 R-type adaptor 留給更後面的電路。
- **不取代 `screamer` / `ts9`**——三顆並存正是 A/B 研究的重點。
- **不建整顆 SD-1**（輸入緩衝、旁通、電源）——只做削波放大級（同 PRD 020 界定）。
- **不做電晶體 / 三極管 root**——那是深水區下一題（triode 前級）。
- **不追 SPICE 位元級對拍**——目標：（a）靜態轉移曲線與小訊號增益在容差內、
  （b）**非對稱**（偶次諧波、非零 duty）與 **mid-hump 由電路湧現** 可量測地成立、
  （c）聽感像真 SD-1、與對稱的 screamer 明顯不同。

## 4. 驗收標準

1. **`cargo test`：**
   - **substrate（`blocks::wdf`）**：`AsymDiode` 解滿足 `a = v + R·i(v)`；
     `m=k=1` 與 `DiodePair` 數值一致；`m≠k` 時 `v(−a) ≠ −v(a)`（非對稱）、且對
     對稱正弦輸入有非零 DC（一半削得早）；±1e6 狂推有界 finite；
     `parallel_root_with_source(_, 0)` 等於 `parallel_root`，注入正電流抬高節點電壓。
   - **sd1 core**：增益腳讓削波**頻率相依**（同振幅下中頻比低頻破更多——mid-hump
     由電路湧現，memoryless 做不到）；回授削波**非對稱**（偶次諧波 / 非零 mean）；
     silence→silence（reset 後）。
   - **sd1 全踏板**（沿用家族 suite + 專屬）：`sd1` 產生偶次諧波（h2 明顯，
     對比對稱的 `ts9`/`screamer`）；mid-hump（低音比中音乾淨）；registry captions、
     unity（drive 5/tone 5/level 6，±6 dB、spread <5 dB）、finite/bounded、
     DC-block、多 rate/block、param 平滑、model-switch finite——皆自動涵蓋。
2. **`cargo bench`：** `drive_sd1_4x_oversampled` 每 64-frame stereo block 成本，
   預期與 screamer 同量級（含 Newton + 4× OS），記入 benchmarks 深水區段。
3. **`assert_no_alloc`：** select 到 `sd1` 並狂推全程無配置。
4. **耳朵驗收（使用者）：** `sd1` vs `screamer` vs `ts9` A/B——SD-1 的非對稱破音
   質地（偶次諧波、比對稱 TS「更開」）、mid-hump（低把位厚、roll 回吉他音量收乾）、
   drive/tone 交互；不炸、不混疊；plugin 重跑 clap-validator 確認 `drive_sd1_*` 出現。
