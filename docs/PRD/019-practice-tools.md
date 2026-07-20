# PRD 019: 練習工具組 — 節拍器 → 鼓 Groove → Song Player

狀態：**草案（待開發）**
日期：2026-07-20
里程碑：M22（2026-07-20 路線圖第 9 項，**分三期**）
關聯：PRD 012（全域 tempo——節拍器/鼓的時鐘源）、PRD 018（pitch shifter
——song player 移調共用）、PRD 003（輸出級——monitor mix 落點）、白皮書
「成功長什麼樣」（個人練習情境）

## 1. 背景與決策

AmpliTube standalone、Spark/Katana 類產品的殺手鐧是練習工具。Lion-Heart 有
了全域 tempo（PRD 012）與 pitch shifter（PRD 018）後，這些能低成本兌現。

拍板：**分三期，全部是 monitor/aux 混音，非 chain slot，standalone 專屬**：

- **不進效果鏈**：click/鼓/伴奏**不該被 amp 破壞**——在輸出級（安全 limiter
  之後、送裝置之前）與處理後吉他**平行相加**，各自電平。session 控制側
  解碼/合成，寫入 `rtrb` 環，輸出級排空相加（新增一個 aux producer，比照
  spectrum tap 反向——這次是「灌入」而非「抽出」）。
- **plugin 無此功能**：host 有自己的節拍器/軌道；standalone-first。

## 2. 規格（三期）

**Phase 1 — 節拍器（M22a）**：
- click 合成（短 enveloped 正弦/雜訊 burst），掛全域 tempo（PRD 012）。
- 拍號（4/4 等）、beat 1 重音、count-in、音量。
- GUI：header/live 一顆節拍器開關 + BPM（共用 tempo chip）+ 拍號；REPL
  `metronome on|off`、`click <vol>`。

**Phase 2 — 鼓 Groove（M22b）**：
- 內建鼓 loop 樣本播放（bundled WAV，或使用者 `~/.lion-heart/grooves/`）。
- 掛全域 tempo：固定拍速 loop 用 WSOLA time-stretch 對到當前 BPM（見
  Phase 3 共用），或分速度檔選最近。
- pattern/style 選單、fill、音量。GUI：groove 選單 + play/stop。

**Phase 3 — Song Player（M22c）**：
- 載入 WAV/MP3（**新依賴 `symphonia`**——純 Rust、permissive；解碼在
  player 執行緒**非 audio thread**，RT 無涉，但仍依政策讀其解碼路徑）。
- A-B 段落循環、**變速不變調**（WSOLA 手寫 time-stretch）、**±半音移調**
  （重用 PRD 018 的 grain shifter）、混音電平。
- GUI：song player 面板（波形/進度、A-B 標記、速度/移調滑桿）。

**共用基礎**：
- **aux monitor mix**：輸出級新增 aux 輸入環 + 相加（節拍器/鼓/伴奏合成後
  的立體聲）。各源自己的電平；aux 總線在安全 limiter **之後**相加（伴奏
  不該被吉他的 limiter 壓；aux 自身保守電平不破）。
- **WSOLA**（Phase 2/3 共用）：波形相似疊接 time-stretch，變速不變調；純
  離線可測。

## 3. 非目標

- MIDI 鼓機 / 可程式編曲 / 匯出——這是練習陪練，非編曲工具。
- 伴奏自動移除人聲 / stem 分離。
- 進 chain / 被效果處理（aux 是乾淨 monitor 混音，刻意繞過鏈）。
- 節拍器/鼓進 preset（屬環境，非音色；設定 app-global）。

## 4. 驗收標準

1. `cargo test`：click 對齊 tempo（相位/重音正確）、count-in 拍數、WSOLA
   變速不變調（輸出時長縮放但基頻不變、±cents）、A-B 循環邊界、aux 混音
   相加正確（各源電平、aux 0 = 位元透明原輸出）、symphonia 解碼 WAV/MP3
   往返一段已知訊號。
2. `assert_no_alloc`：aux 相加在 audio thread 無配置（解碼/合成在 player
   執行緒預先填環）。
3. 手動（Mac）：節拍器跟 tempo、count-in 進場；鼓 groove 對拍；載一首歌
   A-B 慢速練 solo（不變調）、移調到自己的調；伴奏不被 amp 破壞、不被
   limiter 抽吸。
4. 分期交付：每期獨立可用（節拍器先落地即有價值，不必等 song player）。
