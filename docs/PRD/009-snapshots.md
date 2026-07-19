# PRD 009: Snapshots — 同板場景切換（含 morph）

狀態：**草案 → 開發中**
日期：2026-07-19
里程碑：M13
關聯：PRD 001（per-pedal 值）、PRD 002（動態鏈）、白皮書 §7 M8+
（「snapshot morphing」深水區項）、白皮書 §4.1（現場場景）

## 1. 背景與決策

現場真正的痛點不是「換 preset」，而是同一首歌 verse→chorus→solo 只想
**改幾個值**（gain 高一點、delay mix 大一點、開 boost），不想重建板面。
現在唯一手段是切 preset，要走 reconcile + 資產 remount + 全鏈重建——
太重，且尾音會斷。

拍板：preset 內長出至多 **4 個 snapshot（A/B/C/D）**，每個是「同一板面
結構上的值覆蓋層」。切 snapshot 只送既有的 `SetParam`/`SetActive`——
**引擎零改動**，尾音天然完好（不動 order、不 install/remove），切換
天然無爆音（全過既有平滑層）。這是把白皮書 M8+ 的 snapshot morphing
提前落地。

## 2. 模型（lh-core，preset schema v6）

**Snapshot = 場景覆蓋層**，per chain slot（以 handle 定址：`drive`、
`drive2`）記兩件事：

- `active`：該 slot 是否啟用。
- `values`：**當前選定 pedal** 的 param 值（real，param key → value）。

**Snapshot 不能改**：pedal 選擇、slot 結構、掛載資產。要換 drive 音色
請放兩顆 drive slot（dynamic chain 就是為此存在）——這條由 snapshot
只帶「選定 pedal 的值」在結構上保證。

Preset 新增兩個 optional 欄位：

- `snapshots`：`BTreeMap<"A".."D", Snapshot>`（sparse——可只存 A 和 C）。
- `active_snapshot`：`Option<"A".."D">`，存檔時的當前場景（載入時 morph 0
  套用；無則維持 baseline chain）。

baseline `chain` 語意不變（結構＋每顆 pedal 完整記憶）。snapshot 疊在
其上。schema **v5→v6**：無 `snapshots` 欄位 = 空 map = 只有 baseline
（等於單一隱含場景），舊檔聲音與行為不變；schema_version 升 6，舊版
build 讀到會明確拒絕（而非默默丟掉場景——符合白皮書「never silently
diverge」）。

## 3. 切換與 morph（純控制側）

切 snapshot：對每個「當前值 ≠ 目標值」的 param 送 `SetParam`，對 `active`
差異送 `SetActive`。不動 order、不 install/remove——**delay/reverb 尾音
續響、切換無爆音**。

**Morph time**（app-global `morph_ms`，config.json，0–2000 ms，預設 0）：

- `0`：單批次立即套用（各效果自身的平滑層負責去爆音）。
- `>0`：控制執行緒在 GUI/REPL tick 上，對每個 param 的 **normalized** 值
  從當前線性內插到目標（log range 在 norm 空間才平滑），逐 tick 送
  `SetParam`——場景之間是一道可聽的掃移（filter 掃、mix 淡入）。
- `active` 翻轉在 morph 起點即送（引擎 `SetActive` 自帶 crossfade）。

morph 數學（diff 產生 step 表、norm 內插、收斂）抽成純函式單元測試；
session 薄層把它接到真 ChainHandle。切 snapshot 一律 `midi_desync_all`
——pickup 控制器（PRD 008）須重新咬合。

## 4. GUI

preset bar（◀ picker ▶ + save-as）右側加 **4 顆 A–D chip**：

- 點擊：切到該場景（morph）。
- ⌥-click：把當前值存入該格（capture）。
- 當前場景 chip 亮 tube-amber；已存有內容的格子實心、空格子虛線。
- **dirty 點**：當前值偏離當前場景已存的值時亮一點（提示「未存」）。

存 preset 時連同 snapshots 一起寫；載入時還原並套用 `active_snapshot`。

## 5. MIDI

`cc` 表支援虛擬目標 `"snapshot"`（bare-slot 形式的延伸）：value 四等分
選 A–D。復用 PRD 008 的 learn（右鍵旋鈕不適用；此目標手改 JSON 或用
REPL `set snapshot A`）。REPL：`snapshot <A-D>`（切）、`snapshot save <A-D>`
（存）、`morph <ms>`（設 morph 時間）。

## 6. 非目標

- **Plugin v1 不做**——host automation 與 plugin 內場景切換語意衝突，
  DAW 使用者本就有 automation lane。standalone first（ADR 記）。
- snapshot 命名（固定 A–D，不取名）。
- 每 snapshot 獨立資產/pedal/結構（明確排除，見 §2）。
- GUI morph 滑桿（v1 用 config + REPL 設；GUI 控制是後話）。

## 7. 驗收標準

1. `cargo test`：v5→v6 遷移恆等（舊檔載入結構/值不變）；snapshot
   store/load round-trip；diff 正確性（當前 vs 目標只動有差異的 param）；
   morph 內插單調收斂（t=0 起點、t=1 目標、中點在之間）；切場景後
   pickup 解除同步。
2. 舊 preset（v1–v5）載入零影響（無 snapshots）。
3. 耳朵驗收（使用者）：verse/chorus/solo 三場景現場切換，尾音續響、
   無爆音；morph 設 1 s 聽 filter/mix 掃移；⌥-click 存、點擊切、dirty
   點正確；preset 存/載保留全部場景；MIDI 切場景。
