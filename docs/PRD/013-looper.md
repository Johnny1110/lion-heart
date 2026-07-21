# PRD 013: Looper — 鏈上任意位置的循環錄音

狀態：**已實作（ADR 019）**
日期：2026-07-20
里程碑：M16（2026-07-20 路線圖第 3 項）
關聯：PRD 002（動態鏈——slot 是實例，位置決定錄乾/濕）、PRD 012（全域
tempo——量化的來源，v2）、白皮書 §3.1（RT 規則）、§4.2（click-freeness）

## 1. 背景與決策

競品全都內建 looper（QC 4:44、Helix、AmpliTube）。Lion-Heart 缺。使用者
要一顆能錄/疊/撤銷的循環器，用來練習與寫歌。

拍板：**looper 是一個 chain slot 家族（單踏板）**，非 DEFAULT_CHAIN
成員（`add looper` 加入）：

1. **位置即語意**（PRD 002 已給）：把 looper 拖到 drive 前錄乾訊號、
   拖到 cab 後錄完整處理音——這正是 QC「放格線任意位置」的價值，動態
   鏈免費提供，不需新機制。
2. **引擎零改動**：transport（錄/放/疊/撤銷/清除）走既有 `set_param`
   路徑——每個動作是一個 **momentary 參數**（值跨 0.5 觸發，效果內部邊緣
   偵測，同 PRD 012 `tempo.tap` 手法）；連續參數 `level` 走一般平滑。
   engine/session 不加訊息。
3. **RT 安全**：60 秒立體聲緩衝在 `prepare()` 一次配置（`prepare` 是唯一
   准許配置的地方，RT 規則）——`60 × sample_rate × 2 buf` ≈ 46 MB。撤銷
   靠**雙緩衝指標交換**（committed / working），非 audio thread memcpy。
   錄音寫入、疊加 sum-in、播放讀取全在預配置緩衝內；無 alloc、無 NaN、
   feedback-free（疊加增益 ≤ 1 由設計保證有界）。

## 2. 規格

**狀態機**（一顆 momentary `rec` 鈕循環，經典單鈕 looper）：
`empty →[rec] recording →[rec] playing →[rec] overdubbing →[rec] playing
→[rec] overdubbing …`。首次錄音定義循環長度（v1 自由長度，見非目標）。

**Faceplate**：
- `rec`：momentary——推進狀態機。
- `undo`：momentary——working ↔ committed 指標交換（一層撤銷/重做）。
- `clear`：momentary——回 empty，緩衝歸零（下次錄音重設長度）。
- `reverse`：stepped off/on——反向讀取（讀頭倒走）。
- `half`：stepped off/on——半速讀取（緩衝讀率減半 → 表觀長度 ×2、下移
  八度，經典 looper 把戲）。
- `level`：0..1.5 循環播放電平（連續、平滑）。
- `mix`：0..1 乾/循環混音（1.0 = 循環與直通等量疊加）。

**訊號路徑**：`out = dry×(1−fade) + [dry] + loop_playback×level`。錄音時
把輸入寫入 working；疊加時 `working[i] += input[i]`（軟限幅 `tanh` 護頂，
無限疊加不炸）；播放時讀頭以 reverse/half 決定方向與步進，Hann 交叉淡出
loop 接縫（wrap 點消爆音）。

**GUI**：faceplate 上 transport 按鈕列（REC/PLAY·OVERDUB/UNDO/CLEAR，
LED 顯示狀態：紅=錄、綠=放、琥珀=疊）＋ level/mix 旋鈕＋reverse/half chip。
狀態 LED 隨 slot 卡片脈動。**MIDI**：CC 綁 `looper.rec` 等 momentary 參數
（腳踏控制循環），走既有 learn/pickup 免費支援。

**Livery**：looper 家族自己的 signature 色，納入 theme distinct-livery pin。

## 3. 非目標

- **量化（quantize to tempo）**：v2。#4 tempo 已就位，v2 讓 `sync` on 時
  循環長度吸附到小節；v1 先自由長度（首錄定長）。
- 多循環軌 / 無限疊層歷史（v1 一層 undo/redo）。
- 循環匯出成 WAV（#3 recorder 是另案；未來可讓 looper 緩衝存檔）。
- 進 plugin：v1 standalone 專屬（DAW 有自己的循環/凍結；plugin 鏈是
   host-driven）——與 spillover 同理由。

## 4. 驗收標準

1. `cargo test`：狀態機轉移正確（空→錄→放→疊→放）、undo 指標交換還原、
   clear 歸零、reverse/half 讀取正確、疊加軟限幅有界（狂疊不炸/無 NaN）、
   循環接縫無爆音（max sample-to-sample delta 有界）、mix 0 位元透明乾訊、
   多 rate/block、`prepare` 後緩衝足量（60 s @ 96 k 不溢位）。
2. `cargo bench`：`looper` 每 block 成本（錄/放/疊三態）< 0.15 % deadline。
3. `assert_no_alloc`：debug 建置錄/疊/撤銷/清除全程無配置（SIGABRT 即失敗）。
4. 耳朵驗收（使用者）：錄一段和弦→疊主奏→撤銷主奏→重疊；把 looper 拖到
   drive 前錄乾、拖到 cab 後錄濕聽差異；half 半速八度、reverse 倒放；
   腳踏 CC 控 rec 打循環。
