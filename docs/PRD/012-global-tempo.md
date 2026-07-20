# PRD 012: 全域 Tempo — 統一 tap、MIDI clock、plugin host BPM 同步

狀態：**草案 → 開發中**
日期：2026-07-20
里程碑：M15（2026-07-20 路線圖第 2 項）
關聯：PRD 004（delay 家族與 tap——per-slot tap 的來源）、PRD 008
（CC shaping／虛擬 target 前例）、ADR 007（subdivision = 控制側修飾）

## 1. 背景與決策

現況三個缺口：tap tempo 是 **GUI 內 per-slot** 的（每顆 delay 各記各的
拍）；standalone 聽不懂 **MIDI clock**（鼓機／DAW 帶不動 delay）；plugin
的 delay time **不跟 host BPM**——DAW 使用者把這視為 bug 等級的缺陷。

拍板：**一個 session 級全域 tempo，三個寫入者，一種讀取者**：

1. **寫入者**：(a) tap——faceplate TAP 鈕全域化＋footer BPM chip＋REPL
   `tap`＋MIDI 虛擬 target `tempo.tap`；(b) **MIDI clock**（0xF8，24
   ppqn；0xFA/0xFC 重置/凍結）；(c) **plugin host transport**（僅
   plugin 側）。不做顯式仲裁——clock 跑著時每 tick 覆寫（事實上勝出），
   停了 tap 立即接手；REPL `tempo <bpm>` 可手動指定。
2. **讀取者**：delay 家族每個 voice faceplate **尾端 append `sync`**
   （stepped off/on，預設 off）。全域 tempo 更新時，session 對 sync on
   的每顆 delay 實例重導出
   `time = 60000/bpm × subdivision_ratio`（clamp 進 voice 範圍）。
3. **引擎與 DSP 音訊路徑零改動**：`sync` 與 `subdivision` 同款——參數
   存在、進 preset、進 plugin，但音訊路徑視為 no-op（ADR 007 模式）；
   解析全在控制側（standalone = session、plugin = forwarding 層）。
   append-only → **無 schema bump**、plugin id 純加法。

## 2. 行為規格

- **faceplate TAP**：設全域 tempo；被拍的那顆 slot 即使 sync off 也
  一次性套用（＝舊行為不變）；其他 sync on 的 delay 自動跟上。
- **subdivision / sync 撥動**：若已有全域 tempo，立即重導出該 slot 的
  time（sync 撥 on 的瞬間就對拍）。
- **載入 preset**：time/sync/subdivision 照存照載；載入後**不**主動重套
  全域 tempo（preset 裡的 time 是作者意圖）——下一次 tempo 事件才接管
  sync on 的 slot。全域 BPM 本身不持久化（time 參數就是持久化結果）。
- **MIDI clock**：用 midir 時戳（µs）算 tick 間隔（drain 批次化不影響
  精度）；取近 24+ tick 的**中位數**；異常間隔（>±[4, 120] ms 窗）視為
  斷流重啟；BPM 變化 <0.5% 不重寫 time（遲滯防抖）；0xFA 清 tick 相位、
  0xFC 停止累積（BPM 凍結在最後值）。
- **plugin**：`sync` on 且 host 有 tempo → 每 block 從
  `transport().tempo` 導出 time 直寫 chain，host 的 `time` 參數**被忽略**
  （業界慣例：sync 亮著 ms 鈕無效）；sync 轉回 off 時重新套用 host time
  參數值。host 無 transport tempo → 不動。
- **GUI**：footer 左側新 BPM chip（`♩ —`／`♩ 120`，點擊＝tap）；delay
  faceplate 的 TAP 行改讀全域 BPM。per-slot `TapState` 從 GUI 移進
  session（tap 數學不變：2 s timeout、近 4 段平均）。
- **MIDI**：`tempo.tap` 虛擬 target（value ≥ 64 的緣觸發＝一踩；hand-edit
  `midi.json`，同 PRD 008 volume-pedal curve 前例）。
- **REPL**：`tap`（一踩）、`tempo`（顯示）、`tempo <bpm>`（手動 30–300）。

## 3. 非目標

- mod／tremolo rate sync（下一批；tempo source 這次就位）。
- MIDI clock **out**、PPQ 相位鎖定（echo 對齊 transport 小節位置）、
  per-preset BPM 欄位。
- `tempo.tap` 的 GUI learn 綁定（learn 走 chain 參數驗證，虛擬 target
  之後另補）。

## 4. 驗收標準

1. `cargo test`：clock 數學（ticks→BPM、中位數抗抖、gap 重啟、
   start/stop）、tap 數學搬遷後行為不變、sync/subdivision 重導出與
   clamp、delay 參數 pin 測試更新（6/8/7 鈕）、realtime bytes 解析、
   plugin 導出時間純函式。
2. 引擎零 diff；`assert_no_alloc` 不受影響（無音訊路徑改動）。
3. 手動（Mac）：(a) faceplate tap → 第二顆 sync on 的 delay 同步跳動；
   (b) 鼓機/DAW 送 clock：BPM chip 鎖定顯示、轉 subdivision 重新對拍、
   停 clock 後 tap 立即接手；(c) plugin 掛在 host：sync on 跟 project
   BPM，host 自動化 time 無效，sync off 恢復；(d) `tempo.tap` 綁在
   momentary 開關上可用。
