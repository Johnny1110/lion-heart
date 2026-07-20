# PRD 011: Parametric EQ 踏板 — 鏈上的全參數 EQ

狀態：**草案 → 開發中**
日期：2026-07-20
里程碑：M14
關聯：PRD 001（per-pedal 參數）、PRD 002（動態鏈——「任何位置」的機制）、
PRD 003（全局 EQ——DSP 核心與 UI 的來源）

## 1. 背景與決策

鏈上的 `eq` slot 只有 post-amp 三段（low shelf / 可掃 mid / high shelf），
音色塑形夠用，但使用者要在**鏈上任何位置**使用全局 EQ 等級的 8-band
可視化 parametric EQ（拖拽手柄、響應曲線、任意 band type）。

拍板：**`eq` 家族從單踏板變雙踏板**（M9 delay 的家族化路線，但不改名、
不遷移——3-band 踏板 key `eq` 原地保留，append `parametric`）：

1. 「任何位置」不需要任何新機制——動態鏈（PRD 002）已允許把 eq slot
   拖到任意位置、`add eq` 加第二第三顆（`eq2`、`eq3`，上限 12 slots）。
   多實例各自獨立記憶，preset 全存。
2. DSP 直接重用 `lh_dsp::eq::global::GlobalEq`（PRD 003）：per-band wet
   crossfade、log-domain freq 平滑、settled-skip 係數重建、全 off 位元
   透明——全部繼承，不重寫第二份。slot bypass 由引擎既有 crossfade
   承擔；核心的 master 常駐 1.0，不曝光。
3. UI 照搬全局 EQ 面板：`EqPanel` canvas（拖 = freq/gain、滾輪 = Q、
   雙擊 = 啟停）與 detail strip 改為雙用（target = Global / Slot）。
   頻譜疊加沿用**輸出級 tap**（v1 不做 per-slot tap）；面板標示
   「OUT」示明量測點，避免誤讀。
4. 參數化：8 band × 5 參數（on / type / freq / gain / q）= 40 個
   `ParamDesc`，走一般參數路徑——per-pedal 記憶、preset、REPL
   （`eq.b3_freq 250`）、MIDI learn、scenes/morph、plugin 展開全部
   免費取得。
5. **無 schema bump**：append-only 詞彙（M11/M12 前例）。舊 preset 載入
   後 eq slot 停在 3-band、值不變；parametric 的初值 = 全 band off
   （平直、位元透明）。

## 2. 規格

**參數**（band `b1`..`b8`，每 band 5 個）：

- `b{n}_on`：stepped off/on，預設 off。
- `b{n}_type`：stepped low cut / low shelf / bell / high shelf / high cut
  （順序 = `lh_core::global_eq::BandKind::ALL`）。
- `b{n}_freq`：20 Hz–20 kHz log。
- `b{n}_gain`：±18 dB（cut 型忽略，同全局 EQ）。
- `b{n}_q`：0.3–18 log。

預設 layout 與全局 EQ 相同（low-cut 30 / low-shelf 80 / bell 250 / 500 /
1.2k / 3k / high-shelf 6k / high-cut 12k，全 off）。

**Faceplate**：不是旋鈕列——board 檢視選到 parametric 時，參數面板渲染
EQ canvas + detail strip（type、freq/gain/q 數值、flat）。曲線由 40 個
參數即時合成（`response_db`，與音訊路徑同一套 RBJ 數學——畫的即真相）。

**DSP**：`eq/parametric.rs` = 40 參數 →`Band` 映射，包一顆 `GlobalEq`
核心；`eq/mod.rs` 新家族 effect（filter 家族模式）預配置 3-band 與
parametric 兩顆核心，`select_pedal` 換索引 + reset。engine / session /
preset **零程式碼改動**（多踏板路徑既有）。

**Plugin**：param 自動展開（`eq_parametric_b1_on`…）+ `eq_pedal` 選擇器
出現；單踏板家族變多踏板會改既有 eq param id——**pre-v0.1 break**
（M9 前例），重跑 clap-validator。

**Livery**：parametric 有自己的 signature 色，納入 theme 的
distinct-livery pin 測試。

## 3. 非目標

- per-slot 頻譜 tap（要動 RT plumbing；等真的需要再立案）。
- 不動全局 EQ 的任何行為與持久化；全局 EQ 仍不進 preset。
- 不做 mid-side、dynamic EQ、線性相位。
- 3-band 踏板不退場——快速 tone shaping 的低成本選項 + 舊 preset 原味。

## 4. 驗收標準

1. `cargo test`：40 參數映射正確（每 type 響應、freq/gain/q 邊界與
   路由）、全 off 位元透明、engage declick、踏板切換有界且參數路由
   正確、多 rate 多 block、每鈕掃掠有限；registry 一致性（append-only、
   controls 對齊）；livery 相異測試納入 parametric。
2. 曲線=真相：`response_db` 對渲染音訊 < 0.5 dB（沿用全局 EQ 測法）。
3. 舊 preset 載入：eq slot 仍是 3-band、聽感不變；save→load 保留
   parametric 40 值與 active pedal；`add eq` 第二顆獨立記憶。
4. `cargo bench`：parametric settled 成本與 global EQ 同級
   （settled-skip 生效），不因掛在鏈上而回歸。
5. 耳朵驗收（使用者）：切到 parametric 拖 bell ±、雙擊啟停無爆音；
   同一顆 eq 拖到 drive 前 vs cab 後聽位置差；`add eq` 兩顆同板各自
   記憶；MIDI learn 綁 `b3_freq` 掃頻。
