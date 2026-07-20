# PRD 017: Power Amp 模擬 — 後級飽和、Sag 與喇叭壓縮

狀態：**草案（待開發）**
日期：2026-07-20
里程碑：M20（2026-07-20 路線圖第 7 項）
關聯：白皮書 §5.3（NAM 音色核心）、PRD 002（動態鏈）、PRD 007（新家族
+ default_active 旗標前例）、drive 家族（oversample 前例）

## 1. 背景與決策

NAM 生態海量 capture 是 **preamp-only**——沒有後級 sag 與喇叭手感，聽起來
死板。GENOME 靠 TSM power amp 賣這個、Tonocracy 靠 speaker compression 賣
「feel」。這正對 Lion-Heart「音色核心外全手寫 DSP」的定位。

拍板：**新家族 `power`（單踏板 v1），放 amp 之後、cab 之前**，手寫後級：

1. **DEFAULT_CHAIN 11→12**（頂到目前 cap 12）：
   `gate filter comp drive amp **power** eq mod delay reverb cab limiter`。
   同時 **`MAX_SLOTS` 12→16**（給 add 與後續 pitch 家族留位；引擎固定容量
   陣列 `[u8; MAX_SLOTS]` 等只是變大，無邏輯改動）。
2. **預設 bypass**（`default_active("power") = false`，同 filter）：full-amp
   capture（含後級）不要第二層後級，會雙重著色；preamp-only 使用者按亮 LED
   啟用。一面旗，app 預設板與 plugin bypass 參數共用（PRD 007 機制）。
3. **手寫 DSP，4× oversample**（drive 家族前例，削波前升取樣抗混疊）：
   - **push-pull 非對稱飽和**：class-AB 上下管不對稱 + 交越區——偶次諧波與
     後級「肥」。
   - **Sag**：輸出電平 envelope follower 調變可用增益/餘裕——大訊號時供電
     下垂、觸弦「彈回」的動態壓縮（後級靈魂）。
   - **Presence**：負回授路徑高頻 shelf（提升高頻臨場）。
   - **Depth/Resonance**：低頻 shelf（後級低頻共振/緊實）。
   - **輸出變壓器**：低頻 rolloff + 輕微鐵芯飽和。

## 2. 規格

**Faceplate（5 鈕）**：`drive`（後級推進量 0..1）/ `sag`（下垂深度 0..1）/
`presence`（高頻 shelf ±dB）/ `depth`（低頻 shelf ±dB）/ `master`（電平，
共用 level 法則）。

**訊號路徑**：input → depth/presence 前塑 → 4× oversample → push-pull
非對稱 waveshaper（sag envelope 調變 knee/gain）→ 降取樣 → 輸出變壓器
低頻塑形 → master。立體聲雙路獨立狀態。sag 的 envelope 是每聲道或 linked
（linked 較真實——共用供電；比照 gate/comp linked detector）。

**Livery**：power 家族 signature 色，納入 distinct-livery pin。plugin
自動展開 `power_*` 參數 + `power_active`（預設 off，pin 測試）——**pre-v0.1
id 新增**，重跑 clap-validator。

## 3. 非目標

- 選管型（EL34/6L6/KT88 切換）——v1 固定一種 push-pull 音聲；管型選單 v2。
- 真實電路級（WDF）後級——行為級手寫（sag/飽和/變壓器的**感覺**，非電路）。
- 整流器選擇（tube/solid-state rectifier sag 差異）——sag 一鈕含括。
- 進 amp slot（amp 是 NAM singleton；power 獨立 slot 更乾淨）。

## 4. 驗收標準

1. `cargo test`：drive 推進增加諧波、sag 動態壓縮（大訊號輸出/輸入比 <
   小訊號，觸弦回彈可量）、presence/depth shelf 響應正確、非對稱產生偶次
   諧波（h2 顯著）、oversample 抗混疊（高頻輸入無混疊偽頻）、bypass 位元
   透明、預設 bypass（default_active + plugin 參數 pin）、DEFAULT_CHAIN 12
   槽 pin（app/registry/plugin 三方）、`MAX_SLOTS` 16 後引擎陣列邏輯不變
   （既有引擎測試全綠）、多 rate/block、狂推有界無 NaN。
2. `cargo bench`：`power`（含 4× oversample）與 drive 踏板同級（~10 µs）。
3. `assert_no_alloc`：啟用/推滿全程無配置。
4. 耳朵驗收（使用者）：preamp-only NAM 掛 power 後「活過來」——觸弦 sag
   彈性、後級肥、presence/depth 塑形；full-amp capture 確認預設 bypass、
   按亮才著色；不炸不混疊。
