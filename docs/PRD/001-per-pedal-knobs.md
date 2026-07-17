# PRD 001: 效果器獨立旋鈕與參數記憶

狀態：**草案 → 開發中**
日期：2026-07-17
里程碑：M8（本 PRD 與 002、003 構成 M8「自由踏板箱」）
關聯：ADR 003（drive model registry，本 PRD 部分取代其決策）、白皮書 §4.3

## 1. 背景與問題

ADR 003 為了 plugin 參數表穩定與實作簡化，把 drive 家族做成「**一份共用
參數表 + stepped `model` 參數**」：`lh_dsp::drive::PARAMS` 固定七欄
（model, drive, tone, level, low, mid, high），所有 model 共用同一組
knob 定義與 smoother。modulation 家族同構（type + rate/depth/feedback/mix）。

evva（五旋鈕設計）加入後，這個結構的兩個缺陷變成實際痛點：

1. **旋鈕繼承污染**：evva 的 Low/Mid/High 進了共用表，TS9、Blues Driver
   等三旋鈕踏板也「繼承」了這三顆參數。GUI 靠 `model_knob_name()` 對
   `model == 4` 特判遮掉，但 REPL、MIDI、plugin 全都看得到七個參數；
   evva 自己還掛著一顆無作用的 tone 旋鈕。每加一顆非標準面板的踏板，
   特判就多一層。
2. **切換時參數跟著跑**：smoother 是 slot 級共用的，切 model 只換電路
   不換值——像換了踏板卻把上一顆的旋鈕位置原封搬過來。使用者需要的是
   每顆踏板記住自己的調校。

## 2. 目標

1. **除 amp 與 cab 外**，每一顆具體效果器（下稱 **pedal**）擁有完全獨立
   的參數表：數量、名稱、單位、範圍、預設值、平滑時間各自定義，彼此零
   依賴。TS9 恰好三顆旋鈕，evva 恰好五顆。
2. 每顆 pedal 的參數值在 runtime **各自記憶**：同一 slot 內切走再切回，
   旋鈕停在離開時的位置。
3. Preset 記錄 slot 內**所有** pedal 的參數（含當前選用哪顆），載入即
   完整還原，包括未選用踏板的調校。
4. 下游全部由 registry 驅動：GUI 旋鈕與下拉、REPL 標籤、MIDI 映射、
   plugin 參數。新增一顆 pedal = 實作 DSP + 註冊一筆定義，不碰下游。

## 3. 非目標

- amp（NAM capture）與 cab（IR）維持資產驅動 + 固定旋鈕，不納入 pedal
  體系（使用者明確排除）。
- 不改變任何既有 pedal 的**聲音**：DSP 演算法與校準不動，只動參數繫結。
- plugin 的動態參數表：host 會快取參數清單，仍不可行；改以「全 pedal
  參數靜態展開」解（見 §7.6）。

## 4. 使用者故事

- 我選 TS9 時面板只有 Drive/Tone/Level 三顆旋鈕，跟真踏板一樣；切到
  evva 變成 Gain/Low/Mid/High/Level 五顆。
- 我把 TS9 的 drive 調到 7，切去試 evva 又切回來，drive 還在 7。
- 我存了 preset，裡面 TS9 和 evva 的調校都在；換台電腦載入完全一樣。
- 我寫新踏板時只要實作 `Circuit` + 註冊參數表，GUI/REPL/MIDI/plugin
  自動長出正確的旋鈕。

## 5. 概念模型（lh-core）

```
FamilyDesc  ── 一個鏈位分類（drive、mod、gate、…）
  └── pedals: &'static [&'static EffectDesc]   // 1..N 顆
EffectDesc  ── 一顆 pedal 的描述（key/name/params，結構不變、語意降級）
```

- 單 pedal 家族（gate/comp/eq/delay/reverb/limiter/amp/cab）：pedals
  長度 1，pedal key = family key。
- `Effect` trait 增補：
  - `family() -> &'static FamilyDesc`
  - `pedal_index() -> usize`、`select_pedal(usize)`——RT-safe：所有
    pedal 電路建構時預配置，切換是索引切換 + 新 pedal 狀態 reset
    （與 ADR 003 同契約：短暫不連續、絕不配置記憶體、絕不 NaN）。
  - `descriptor()` 回傳**現用 pedal** 的 desc；`set_param(index, norm)`
    以現用 pedal 的參數表為索引空間。
- **參數記憶的所在層：控制端 `ChainHandle` 的 shadow**（slot → pedal →
  norms）。切 pedal 時控制端送 `SelectPedal` 後緊接重送目標 pedal 的全部
  參數（≤ 8 則訊息，SPSC ring 順序保證正確性）；效果器內部**不需要**
  per-pedal 值儲存。記憶天然存在 shadow 與 preset。

## 6. 各家族 pedal 參數表

| family | pedal | 參數（順序即索引） |
| --- | --- | --- |
| drive | ts9 | drive, tone, level（0–10） |
| drive | blues driver | gain, tone, level |
| drive | classic | drive, tone, level |
| drive | centaur | gain, treble, output |
| drive | evva | gain, low, mid, high, level（移除無作用的 tone） |
| mod | chorus | rate, depth, feedback, mix |
| mod | flanger | rate, depth, feedback, mix |
| mod | phaser | rate, depth, feedback, mix |
| mod | tremolo | rate, depth（mix 對 tremolo 與 depth 重複，移除） |
| 其他 | （單 pedal） | 現有參數不變 |

rate 等範圍可依 pedal 各自調整（例：tremolo 0.5–12 Hz）——「各自可調」
正是本 PRD 的目的；實作時定案，測試鎖行為。

## 7. 技術設計

### 7.1 lh-dsp

- `drive`：`ModelDef` 增加 `desc: &'static EffectDesc` 與參數→內部控制
  的映射（drive/tone/level/low/mid/high smoother 依 pedal 參數位置繫結）。
  刪除 `model_knob_name()` 特判——captions 直接來自 pedal desc。
- `modulation`：四種 type 提升為四顆 pedal desc；DSP 演算法不變。
- 其他效果器：包一層單 pedal family（機械性改動）。

### 7.2 lh-engine

- `EngineMsg::SelectPedal { slot, pedal }` 新增。
- `ChainHandle`：shadow 變 slot → pedal → norms；`set_param` /
  `snapshot` / `apply` 對現用 pedal 操作；`select_pedal()` 送訊息並重送
  目標 pedal 參數。

### 7.3 Preset schema v3

```json
{ "schema_version": 3, "name": "lead", "chain": [
    { "key": "drive", "active": true, "pedal": "ts9",
      "pedals": { "ts9":  { "drive": 7.0, "tone": 5.0, "level": 6.0 },
                  "evva": { "gain": 4.0, "low": 5.0, "mid": 6.0,
                            "high": 5.0, "level": 6.0 } } }
  ], "assets": { } }
```

- 載入：未提及的 pedal 用預設值；未知 pedal/param 略過 + 警告（沿用
  forward-compat 規則）；未提及 `pedal` 用家族第一顆。
- **v2 → v3 遷移**：
  - drive：`model` 索引 → pedal key；值改名映射——blues driver
    `gain ← drive`；centaur `gain ← drive, treble ← tone,
    output ← level`；evva `gain ← drive`（tone 丟棄）；ts9/classic 原名。
  - mod：`type` 索引 → pedal key；tremolo `depth' = depth × mix`
    （聽感等價：mix 淺化調變深度）。
- v1 檔案先走既有 v1→v2（drive_law 反演、classic 錨定）再 v2→v3——
  **舊 preset 聲音不變**的保證完整保留。

### 7.4 MIDI 與 REPL

- pedal 選擇是虛擬參數 `pedal`：`set drive.pedal ts9`；CC 映射
  `"81": "drive.pedal"`（0–127 均分到 pedal 數）。
- `model`（drive）與 `type`（mod）作為相容別名，指向 `pedal`。
- CC `"slot.param"` 作用於現用 pedal；param 不存在則忽略並回報一行。

### 7.5 GUI

- 旋鈕列與 captions 直接由現用 pedal desc 生成；pedal 下拉由 family
  registry 生成。切 pedal 後旋鈕自動反映該 pedal 的記憶值（shadow）。

### 7.6 Plugin

- 實例化時**靜態展開全 pedal × 全參數**：`drive_ts9_drive`、
  `drive_evva_low`…＋每個多 pedal slot 一個 `pedal` selector（stepped）。
- host 對非現用 pedal 的參數改動僅存值（host 參數本身即記憶）；selector
  切換時 plugin 送 `SelectPedal` + 該 pedal 全參數——與 app 行為一致。
- 參數總數 ~70，host 無壓力。這是 pre-v0.1 的一次性參數表 break
  （ADR 003 已接受同類 break）。

## 8. 相容性與風險

| 風險 | 對策 |
| --- | --- |
| 舊 preset 音色跑掉 | v1→v2→v3 遷移鏈 + 既有 golden 測試延伸到 v3 |
| midi.json 舊映射失效 | `model`/`type` 別名一版緩衝；文件註明 |
| 切 pedal 的重送訊息塞爆 ring | ≤ 8 則/次，ring 容量 256、每 block 汲取 64——餘量充足 |
| plugin 參數 id 變動 | pre-v0.1 接受；ADR 004 記錄 |

## 9. 驗收標準

1. GUI：TS9 顯示三顆旋鈕（Drive/Tone/Level）、evva 顯示五顆
   （Gain/Low/Mid/High/Level），任何 pedal 不再出現無作用旋鈕。
2. 切 ts9 → 調 drive=7 → 切 evva → 調 gain=4 → 切回 ts9：drive 仍為 7
   （GUI/REPL/MIDI 三處一致）。
3. Preset 存檔重載：所有 pedal 的值（含未選用）完整還原。
4. v1/v2 舊 preset 載入聲音不變（遷移單元測試 + drive 家族既有
   character 測試全綠）。
5. `cargo fmt --check`、`clippy -D warnings`、`cargo test` 全綠；
   clap-validator 16/16；null-device `--buffer 32` 在 assert_no_alloc
   下乾淨。
