# PRD 003: 全局輸出 EQ 與即時頻譜

狀態：**草案 → 開發中**
日期：2026-07-17
里程碑：M8（與 001、002 同組）
關聯：白皮書 §3.3（常駐輸出 limiter）、§4.1（garbage chute / tap 模式）、PRD 002

## 1. 背景與問題

鏈上的 `eq` slot 是音色塑形（post-amp 三段），不是輸出修正。使用者需要
在 **output 前**一層常駐的全頻段 parametric EQ，對最終輸出做房間/監聽/
錄音對頻的優化——像專業錄音軟體的 EQ：在折線面板上任意頻段拖拽；並且
要有**兩組重疊曲線**——一組是 EQ 設定（合成響應），一組是演奏時動態
更新的即時頻譜，邊彈邊看邊調。

## 2. 目標

1. **引擎輸出級**（所有 chain slot 之後、DAC 之前）常駐：
   `chain → Global EQ → safety limiter → out`。EQ 全 band 停用時透明。
2. **8 band parametric EQ**，每 band：
   `enabled`、`type`（low-cut / low-shelf / bell / high-shelf / high-cut）、
   `freq` 20 Hz–20 kHz（log）、`gain` ±18 dB（cut 型忽略）、
   `Q` 0.3–18（log；shelf 用固定斜率、cut 為 12 dB/oct）。
   立體聲同參數、雙聲道處理。
3. **EQ 面板**（GUI 新 overlay）：log 頻率軸 20–20k、dB 軸 ±18；
   疊加顯示 (a) EQ 合成響應曲線與 (b) 即時輸出頻譜；band 手柄直接拖拽。
4. **全域持久化** `~/.lion-heart/global_eq.json`：與 preset 無關——
   切 preset 不動全局 EQ（出口修正屬於環境，不屬於音色）。啟動載入、
   變更即存。
5. **safety limiter**（固定 ceiling −0.3 dBFS、fast release）承接白皮書
   §3.3 的常駐保證：PRD 002 之後鏈上 limiter 可被移除，「任何 patch/
   設定/bug 都不得轟監聽」必須由輸出級兜底；EQ boost 過頭同樣被接住。

## 3. 非目標

- 不進 preset、不進 plugin v1（host 有自己的 EQ 與 analyzer）。
- 線性相位 / dynamic EQ / mid-side——留給未來。
- 頻譜不做瀑布圖 / 峰值保持（v1 只做即時曲線）。

## 4. 使用者故事

- 我彈奏時看到輸出頻譜，200 Hz 有一坨轟，我在面板上把一顆 bell 拖到
  200 Hz 往下拉 4 dB，轟聲即時消失，曲線與聽感同步。
- 我換了排練室，調全局 EQ 適應房間；回家切任何 preset，這組修正都還在。
- 我把 EQ 全部雙擊停用，輸出與沒有 EQ 完全一致。

## 5. UX 規格

- header 新增「eq」chip → 主面板切為 EQ 編輯器。
- **面板**（iced Canvas，佔滿主面板）：
  - 網格：log 頻率刻度（20/50/100/…/10k/20k）、dB 刻度（±18，0 dB
    水平線加亮）。
  - **即時頻譜**：半透明填色折線，fast-attack / slow-release 平滑。
  - **EQ 響應曲線**：亮色實線（合成 |H|）。
  - **band 手柄**：8 顆圓點。enabled 實心、disabled 空心停在其頻率的
    0 dB 線上。
- **互動**：
  - 拖手柄：x = freq、y = gain（cut 型只認 x）。
  - 手柄上滾輪：Q。
  - 雙擊手柄：啟用/停用。
  - 選取 band 顯示細部列：type 下拉、freq/gain/Q 數值、單 band reset。
  - 面板角落：EQ 總開關、「flat」（全部回預設）。
- 8 顆 band 常駐（無增刪），預設頻率分佈 30/80/250/500/1.2k/3k/6k/12k，
  全部 disabled——初始完全透明。

## 6. 技術設計

### 6.1 DSP（lh-dsp）

- 新模組 `param_eq`：RBJ biquad 串（沿用 `biquad.rs`，補
  `set_lowpass`/`set_highpass`，Butterworth Q 起步）。
- freq/gain/Q 走 smoother，**每 block 重算係數**（既有 eq slot 同模式；
  8 band × 雙聲道在 48 kHz/32 frames 額外 ~1–2 µs，佔預算 <0.5%）。
- disabled band 完全跳過（零成本）。
- 響應曲線公式與 RT 係數同一套（`|H(e^jω)|` 求值函式放 lh-dsp，控制端
  取樣 ~240 個 log 頻點）——**畫的曲線即真相**。
- 單元測試：各 type 響應（`gain_at` 模式）、silence→silence、
  多 rate（44.1/48/96k）多 block（32–1024）、全 disabled null test。

### 6.2 引擎（lh-engine）

- `Chain` 增設輸出級：slot 迴圈 + master fade 之後依序
  Global EQ → safety limiter → 輸出 telemetry / tap。
- 新訊息：`SetEqBand { band, field, value }`（field = enabled/type/
  freq/gain/q）、`SetEqActive(bool)`。
- **頻譜 tap**：輸出級之後 mono-sum 寫入 rtrb ring（drop-on-full，
  絕不阻塞 callback——同 tuner tap 模式）。
- safety limiter 重用既有 `Limiter` DSP，參數固定（−0.3 dBFS /
  release 60 ms），不曝光為可調 slot。

### 6.3 GUI 頻譜

- 幀迴圈 drain tap → 4096 樣本滑窗 → Hann → `realfft`（app 依賴，
  控制執行緒，與 RT 無涉）→ 幅度 dB。
- 每 bin 平滑：attack 即時、release ~12 dB/s；顯示 −90–0 dBFS。
- 更新 ~30 Hz（每 2 幀），重繪走既有 canvas cache 模式。

### 6.4 持久化

- `~/.lion-heart/global_eq.json`：serde 結構 = ChainHandle shadow 同
  型別（8 band 真實值 + master enabled）。啟動載入套用；任何變更即存
  （與 config.json 同 write-through 模式）。
- `Session::resume`（裝置/buffer 重啟）自動重套——與 chain carry-over
  同路徑。

## 7. 相容性與風險

| 風險 | 對策 |
| --- | --- |
| 係數重算造成 zipper / 不穩定 | 參數平滑 + block 級重算（既有 eq slot 已驗證此模式）；係數不插值 |
| 頻譜 FFT 佔用 UI 幀預算 | 4096-point realfft ~數十 µs，30 Hz 節流；實測 60 fps 驗收把關 |
| tap 寫入影響 callback | drop-on-full、chunk write（tuner tap 已驗證模式） |
| EQ boost 推爆輸出 | safety limiter 兜底（驗收 5） |
| 舊行為改變（limiter 後多了一級） | 全 band disabled + safety 只在超過 −0.3 dBFS 時作動 → 正常訊號位元級不變（驗收 2） |

## 8. 驗收標準

1. 面板拖一顆 bell +6 dB @ 1 kHz：聽感、響應曲線、頻譜三者同步反映；
   拖動/改 Q/換 type 過程無爆音、無 zipper。
2. 全 band disabled 時 null test：有無 Global EQ 的輸出差 < −120 dB
   （safety limiter 對 −0.3 dBFS 以下訊號透明）。
3. 演奏中頻譜 ~30 Hz 更新，UI 維持 60 fps，xrun 不增。
4. 重啟 app：`global_eq.json` 還原；切換任意 preset 不影響全局 EQ。
5. EQ 全 band +18 dB、全鏈狂開實測：輸出峰值永不超過 −0.3 dBFS。
6. `--buffer 32` null-device 全程 assert_no_alloc 乾淨；全 gates 綠。
