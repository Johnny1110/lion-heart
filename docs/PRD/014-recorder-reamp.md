# PRD 014: 錄音與 Re-amp — DI + 濕訊同錄、離線重過

狀態：**草案（待開發）**
日期：2026-07-20
里程碑：M17（2026-07-20 路線圖第 4 項）
關聯：白皮書「成功長什麼樣」#1（用 Lion-Heart 錄完一首歌的吉他軌）、
PRD 003（輸出級 tap 機制）、依賴政策（`hound` 已在依賴樹）

## 1. 背景與決策

白皮書第一條成功指標「用 Lion-Heart 完成一首歌的吉他軌」**至今零落地**。
AmpliTube 5 standalone 內建 8 軌 recorder。使用者需要：一鍵錄音 + 事後
換音色（re-amp）。

拍板：**兩件事，共用離線基礎**：

1. **即時同錄兩軌**：錄音時同時寫 **DI（乾輸入）** 與 **處理後（濕）**
   立體聲 WAV。DI 保留 = 事後任意 re-amp 的底片。
2. **離線 re-amp**：新 CLI `render <di.wav> --preset <p>`——把任何 DI 檔
   離線重過任一 preset。**效果全部可離線跑**（現有架構鐵律：`Effect` 是
   純 buffer-in/out），所以 re-amp 幾乎免費——同一條鏈，只是餵檔案而非
   裝置。

**技術決策**：

- **兩個新引擎 tap**（比照 PRD 003 spectrum tap，但立體聲、交錯）：DI tap
  在 `Chain::process` 進入時（任何 slot 前）複製 left/right；wet tap 在
  輸出級之後。各自 `rtrb::Producer<f32>`（L/R 交錯），drop-on-full——RT
  絕不阻塞。緩衝設數秒大環，磁碟寫入執行緒快速排空，drop 視同 xrun 級
  事件並計數/告警（掉樣＝錄音瑕疵，不是無聲失敗）。
- **磁碟寫入執行緒**：專屬執行緒（非 audio、非 GUI）排空兩環、交錯、經
  `hound` 寫 `di.wav`/`wet.wav`。engine 只多兩個 tap producer（`set_di_tap`
  /`set_wet_tap`，同 `set_output_tap` 模式），無新 EngineMsg。
- **離線 render**：新 subcommand。載 preset（重用 session 的 preset →
  chain 重建）、`hound` 讀 DI、逐 block 過鏈、`hound` 寫輸出。這條路把
  「效果離線可跑」變現金。

## 2. 規格

**錄音**：
- `~/.lion-heart/recordings/<timestamp>-di.wav` + `-wet.wav`，48 kHz/f32
  或 24-bit（設定）。timestamp 命名（控制側取時鐘，非 RT）。
- Session：`start_recording()` / `stop_recording()`（回傳檔案路徑）。
- GUI：header 或 live view 一顆 **● REC** 鈕（錄音中紅點閃、顯示已錄時長
  與掉樣計數）。REPL：`record start` / `record stop`。
- `AppConfig.recordings_dir`（預設 `~/.lion-heart/recordings`）、
  `record_bits`（16/24/32f，預設 24）。

**Re-amp CLI**：
```
lion-heart render <di.wav> --preset <name> [-o <out.wav>] [--tail <secs>]
```
- 預設輸出 `<di>-<preset>.wav`；`--tail` 多跑 N 秒讓 delay/reverb 尾音收完。
- 找不到 preset / 檔案 / rate 不符 → 明確錯誤（NAM rate-lock 沿用既有
  驗證）。純離線、無裝置、無執行緒——CI 可測。

## 3. 非目標

- 多軌 DAW / 編輯 / 疊軌（AmpliTube 那套）——這是 monitor 錄音，不是 DAW。
- 即時 re-amp（換 preset 重放 DI）——先給離線 CLI；GUI re-amp 面板 v2。
- MP3/其他格式輸出（WAV only；輸入亦 WAV，MP3 解碼留給 PRD 019 song
  player 的 symphonia）。
- 錄 MIDI / 自動對齊 / 節拍格線。

## 4. 驗收標準

1. `cargo test`：磁碟寫入交錯正確（往返一段已知訊號位元不失真）、drop
   計數在環滿時遞增、離線 render 純函式（DI→preset→輸出，含 tail 收尾、
   rate 不符報錯）、tap 在 `Chain::process` 正確落點（DI = 未處理輸入）。
2. `assert_no_alloc`：錄音啟停與寫入全程 audio thread 無配置；tap 滿時
   drop-on-full 不阻塞 callback。
3. 手動：錄一段演奏→`recordings/` 出現 di+wet 兩檔，播放 wet = 當時聽感、
   di = 乾訊；`render old-di.wav --preset lead` 產出重過音色、尾音完整；
   長時間錄音掉樣計數維持 0（硬體實測）。
4. 白皮書成功指標推進：能用錄音+re-amp 完成一軌吉他（使用者主觀確認）。
