# PRD 004: Delay 家族化與 Tap Tempo

狀態：**草案 → 開發中**
日期：2026-07-18
里程碑：M9
關聯：PRD 001（每 pedal 一張臉）、PRD 002（動態鏈）、白皮書 §4.2（無爆音）、§4.3（平滑層）

## 1. 背景與問題

`delay` 目前是**單一 pedal 家族**：一條內插讀頭的環形緩衝 + 固定 4 kHz
回授低通 + dry/wet mix，只有 `time / feedback / mix` 三顆旋鈕。使用者要三種
真實 delay 類型的音色與其 signature 旋鈕，並補上四項現代 delay 必備控制：
**tone（回音亮度）、mod depth、mod rate、tap（跟拍定速）**。

架構上這對得上既有的多 pedal 家族模式（`drive` / `mod`）：一個 chain slot
掛一個家族、每 pedal 自己一張臉，engine 的 per-pedal shadow、preset v3 每 pedal
分存、GUI/plugin 自動展開，全部免費繼承。

## 2. 目標

1. **Delay 升級成三 pedal 家族**（家族 key 仍為 `delay`，順序 append-only）：
   - **digital**：乾淨全頻、回授線性不飽和、tone 可到很亮、time 上限最長
     （~2 s）、無調變。臉：Time / Feedback / Mix / Tone。
   - **tape**：回授軟飽和（溫暖、每次重複更暗、自振有界）、兩顆 LFO
     （慢 **Wow** + 快 **Flutter**，預設皆微開 → 那點 chorus 感）。
     臉：Time / Feedback / Mix / Tone / Wow / Flutter。
   - **vintage**：BBD 類比——偏暗窄頻、回授壓縮更重、單顆 **Mod** LFO、
     time 上限短（~600 ms）。臉：Time / Feedback / Mix / Tone / Mod。
2. **共用新控制**：
   - **tone** 0..1：回授路徑一極低通轉角（每 pedal 對應不同 Hz 範圍），
     暗↔亮，並在回授迴圈內逐次重複遞暗。
   - **mod depth / rate**：以各 pedal 的 signature 旋鈕表達（tape 的
     Wow/Flutter、vintage 的 Mod 皆為**深度**；rate 為各 pedal 固定的音色）。
3. **Tap tempo**：每 pedal 臉上一顆 **TAP 鈕** + `subdivision` 選單
   （stepped 參數，存 preset）。連續點兩下以上算平均間隔得 BPM，
   `tapped_period × subdivision 比例 → delay time`，旁邊顯示 `♩ = BPM`。
4. **Preset 相容**：schema v3 → v4，舊 `delay` pedal 自動改名成 `digital`
   （time/feedback/mix 沿用），舊檔載入不報錯、音色近似（略亮）。

## 3. 非目標

- 無 host-tempo sync（v1 tap 只在 app 內、只算 delay time）。
- Tap **只在 GUI**——不進 REPL、不進 MIDI 腳踏（未來可加）。
- ping-pong / 多 tap / reverse delay——留給未來。
- subdivision 在 plugin 端 inert（plugin 無 tap 鈕；存著讓 app preset 帶得走）。

## 4. 使用者故事

- 我把 delay 切到 tape，聽到重複帶溫暖飽和與一點點 wobble，越後面越暗。
- 我切 vintage，暗、糊、feedback 拉滿變成一團有界的自振 drone。
- 我照歌曲節奏點 TAP 兩三下，回音立刻鎖在拍子上；把 subdivision 撥到
  「附點八分」，回音間隔立刻變成 The Edge 的感覺。
- 我載入 v0.0 存的舊 preset，delay 還在、還響，只是換了張叫 digital 的臉。

## 5. UX 規格

- 板編輯區的 delay 卡片：三 pedal 各自的識別色（digital 冷藍、tape 暖褐、
  vintage 悶青）。
- faceplate：pedal 選單（digital/tape/vintage）→ 對應旋鈕列即時重繪；
  `subdivision` 以下拉呈現（沿用 stepped→dropdown）；旋鈕列下方一顆
  **TAP 鈕** + `♩ = BPM`（未定速時顯示提示）。
- 互動：
  - 點 TAP：控制端記錄時間戳，≥2 下（間隔 < 2 s）平均得 tempo → 設 time。
  - 撥 subdivision：若已有 tempo，即時用新比例重算 time。
  - time 旋鈕：仍可手動覆寫（tap 與旋鈕都只是設同一個 time）。

## 6. DSP 規格

- 一套共用引擎，per-sample loop `match` voice 常數（無 vtable，比照
  `modulation.rs`）：內插讀頭 + 回授 tone 低通 + LFO 讀頭偏移 + 選擇性軟飽和。
- **tone**：回授路徑一極低通，係數 settled 時不重算（對齊 perf 慣例）。
- **回授/自振**：digital feedback ≤ 0.9（線性、恆衰減）；tape ≤ 1.0、
  vintage ≤ 1.05，靠 `tanh(drive·x)/drive` 軟飽和（unity 小訊號、`1/drive`
  天花板）自我限幅——真實類比式自振但恆有限（白皮書 §7 denormal/NaN 規則）。
- **調變**：相位累加器 LFO 偏移讀距（tape 慢 wow + 快 flutter，vintage 單顆，
  digital 無）；右聲道相位差 π/2 給一點寬度。緩衝含 mod headroom。

## 7. 資料 / 相容

- `lh_core::preset::DELAY_PEDALS = ["digital","tape","vintage"]`，由 lh-dsp
  測試 pin 住 ↔ `delay::FAMILY.pedals`。
- v3→v4 migration：delay slot 的 `pedal` 與 `pedals` map 的 `"delay"` 鍵
  改名 `"digital"`。
- Plugin：參數自 descriptor 自動展開，delay 長出每 pedal 參數 + `delay_pedal`
  selector——param id 改變（pre-v0.1 破壞，如同 drive/mod）。

## 8. 驗收

- 三 pedal 音色可辨（乾淨 / 溫暖 wobble / 暗糊自振）、tone 掃亮暗、mod 讓
  尾巴隨時間變化、高 feedback 恆有限、silence→silence、44.1/48/96 kHz。
- Tap 兩下定速、subdivision 即時改間隔、BPM 讀值正確。
- 舊 v3 preset 載入 → digital、值保留。
- 全套 fmt / clippy / test 綠燈；`assert_no_alloc` 無觸發。
