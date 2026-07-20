# PRD 016: 現場強化 — Setlist 與 Preset 響度對齊

狀態：**草案（待開發）**
日期：2026-07-20
里程碑：M19（2026-07-20 路線圖第 6 項）
關聯：白皮書「成功長什麼樣」#4（帶 Mac 上台）、PRD 014（離線 render，
LUFS 量測共用）、PRD 003（輸出級——master trim 落點）、M6 MIDI PC 契約

## 1. 背景與決策

兩個上台痛點：(a) preset 沒有歌單順序（現在只有排序目錄 + prev/next）；
(b) preset 間音量落差是 QC/Helix 論壇長年第一名許願——切 preset 忽大忽小。

拍板：**兩件事，共用 PRD 014 的離線 render**：

1. **Setlist**：把 preset 排成具名歌單。`~/.lion-heart/setlists.json`
   （具名 → preset 名有序清單）。live view 的 prev/next、footswitch、MIDI
   PC 走 **當前啟用歌單** 的順序，而非排序目錄。
2. **響度對齊（LUFS leveling）**：離線把一段參考 DI render 過每個 preset、
   量 integrated LUFS（ITU-R BS.1770 K-weighting，**離線手寫**——無 RT
   限制）、算出打到目標（預設 −18 LUFS）的 master trim，建議並寫入。

**技術決策**：

- **響度偏移 app-global，不進 preset**：`~/.lion-heart/levels.json`
  （preset 名 → trim_db），比照 `global_eq.json`——響度匹配屬「環境」不屬
  「音色」（白皮書 §4.1 精神），且免 preset schema 升版。輸出級新增一個
  **master trim**（安全 limiter 前的增益，比照 global EQ 也在輸出級），
  session 依當前 preset 從 levels.json 套用。
- **MIDI PC 契約保留**：無啟用歌單時，PC n → 第 n 個排序 preset（現況、
  與 plugin 跨二進位契約不變）。啟用歌單時，PC n → 歌單第 n 首——**session
  端覆寫**，plugin 不受影響（plugin 無歌單概念，續用排序索引）。
- **LUFS 量測純離線**：重用 PRD 014 的 `render` 管線把參考 DI 過每個
  preset，K-weighting + gating 積分算 integrated LUFS。純函式、CI 可測。

## 2. 規格

**Setlist**：
- `setlists.json`：`{ "active": "gig-a", "lists": { "gig-a": ["intro",
  "verse-lead", …] } }`。
- GUI：setlist 管理頁（建立/命名/拖序/選啟用；從既有 preset 抓名）；live
  view 標題顯示「歌單名 · 第 3/12 首」，prev/next 走歌單。
- REPL：`setlist <name>`（啟用）、`setlist list`、`setlist add <preset>`、
  `setlist off`（回排序目錄）。
- footswitch：MIDI 既有 PC/CC 綁 prev/next（歌單內移動）。

**響度對齊**：
- CLI：`lion-heart level --preset <name>|--all [--target -18] [--ref <di.wav>]`
  ——量測並寫 levels.json（附建議 trim 報告）。未給 `--ref` 用內建參考 DI。
- 輸出級 master trim：載入 preset 時從 levels.json 取 trim_db 套用（安全
  limiter 仍兜底過大值）；GUI 設定顯示當前 trim、可手動微調。
- GUI「level all」按鈕：背景批次量測（進度顯示），完成後總表 + 一鍵寫入。

## 3. 非目標

- 即時 LUFS 表 / loudness normalization on playback（這是 preset 靜態偏移）。
- 歌單內 per-song 覆寫（移調/踏板狀態）——那是 snapshot（PRD 009）領域。
- 自動抓取線上 setlist / 匯出 PDF 譜面。
- plugin 端歌單（host 的歌曲/場景機制是那邊的答案）。

## 4. 驗收標準

1. `cargo test`：setlists.json 往返與遷移、啟用歌單時 prev/next/PC 走歌單
   順序（無歌單回退排序目錄）、LUFS 積分對已知電平訊號正確（−18 dBFS 正弦
   → 約 −18 LUFS ±容差、K-weighting 加權曲線、gating 忽略靜音段）、trim
   計算（量測 LUFS + 目標 → 偏移 dB）、levels.json 往返。
2. 輸出級 master trim：套用後輸出電平位移正確、trim 0 位元透明、過大 trim
   被安全 limiter 接住（−0.3 dBFS 不破）。
3. 手動（Mac）：建歌單排 5 首、footswitch 走歌單、`level --all` 後切 preset
   音量落差顯著縮小；停用歌單回排序目錄；plugin PC 仍走排序索引。
