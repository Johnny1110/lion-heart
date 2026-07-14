# Lion-Heart 白皮書

| | |
|---|---|
| **版本** | 0.1 |
| **日期** | 2026-07-14 |
| **狀態** | 已定案（設計階段的北極星文件；實作後的偏差以 `docs/adr/` 記錄） |
| **語言** | 白皮書為繁體中文；README、CLAUDE.md、程式碼與註解為英文 |

---

## 1. 摘要

Lion-Heart 是一個開源的電吉他綜合效果器軟體：吉他經錄音介面進入 Mac，由本軟體完成音色塑型（前級破音、音箱模擬、空間與調變效果），再經介面輸出至監聽或 FOH。目標場景是**錄音工作**與**現場演出（取代地板綜效）**。

技術路線一句話總結：**Rust 全棧、獨立 App 優先、NAM + IR 當音色核心、周邊效果全部手寫 DSP、產品級 GUI、插件形態（CLAP/VST3）後置。**

本文件回答三個問題：要做什麼（§2–§3）、怎麼做（§4–§6）、按什麼順序做（§7），並附上風險對策（§8）與成功標準（§9）。

---

## 2. 願景與動機

1. **擁有自己的琴房。** 商業軟體（Neural DSP、Bias 等）封閉且逐漸訂閱化；硬體綜效（Helix、Fractal、Quad Cortex）昂貴且功能固定。Lion-Heart 是一台可以自己讀、自己改、自己擴充的琴房設備。
2. **站在 NAM 生態的肩膀上。** [Neural Amp Modeler](https://github.com/sdatkinson/NeuralAmpModelerCore)（MIT 授權）社群已有上萬個免費、專業品質的音箱 capture 與 cab IR。Lion-Heart 不重造音箱，而是把工程力投入引擎品質、音箱周邊的踏板、與演奏體驗。
3. **邊做邊研究的學習載體。** 開發者本人是後端工程師（Java/Go/Rust）兼吉他手，音訊 DSP 隨專案進度研究。因此除了基礎設施用成熟庫，**每一顆效果器都從差分方程開始手寫**，並在程式碼旁留下研究筆記。
4. **開源交朋友。** 全程開源（MIT OR Apache-2.0 雙授權），目標是累積作品與社群，商業化非目標。

---

## 3. 使用場景與需求

### 3.1 場景

| 場景 | 描述 | 對系統的要求 |
|---|---|---|
| **A. 錄音** | DAW 錄乾訊號、經 Lion-Heart 監聽；或直接錄製處理後訊號 | 音色品質、gain staging 正確、與介面 loopback 工作流相容 |
| **B. 現場** | Mac + 介面上台，取代地板綜效 | 延遲 ≤ 10 ms、一小時零 xrun、preset 無聲切換、tuner、防呆（輸出限幅） |

### 3.2 延遲預算

延遲是本專案的第一性能指標，M0 就要建立量測（loopback 實測 + xrun 計數），一切最佳化以實測為準。

理論預算（48 kHz）：

| 緩衝區大小 | 單向緩衝延遲 | 估計 round-trip（含 ADC/DAC 與驅動開銷 ≈ 2–4 ms） |
|---|---|---|
| 32 samples | 0.67 ms | ≈ 3.5–5.5 ms |
| **64 samples（目標預設）** | **1.33 ms** | **≈ 5–9 ms** |
| 128 samples | 2.67 ms | ≈ 8–12 ms |

- **目標：48 kHz / 64 samples 下 round-trip ≤ 10 ms**（一般吉他手對 ≤ 10 ms 無感，相當於音箱離耳朵 3 公尺）。
- **DSP 預算：** 每個 64-sample 區塊的 deadline 是 1.33 ms。目標平均負載 < 40% 單核、最壞情況 < 80%。參考量級：nam-rs 的標準 WaveNet 推論約 1.9 µs/sample ⇒ 一個 NAM 節點約佔 deadline 的 9%，整條鏈有充足餘裕。
- 現場強化目標（M6）：32 samples 穩定運行。

### 3.3 非功能需求

- **穩定性：** 現場標準是連續一小時零 underrun；引擎必須把 xrun 當一級指標持續統計並可視化。
- **安全：** 輸出端常駐 limiter；啟動時靜音、音量緩升——保護監聽喇叭與耳朵（高增益音色 + bug = 危險）。
- **資源：** 單條完整鏈在 Apple Silicon 效能核上 CPU < 40%；記憶體峰值主要來自 IR/NAM 資產，控制在百 MB 級。
- **可攜性紀律：** MVP 只出 macOS（Apple Silicon 優先），但所選依賴（cpal、iced、nam-rs）皆跨平台，且不在核心寫平台特定程式碼，為 M8+ 的 Windows/Linux 移植留路。

### 3.4 非目標（Non-goals）

明確不做，防止 scope creep：

- MVP 期不做 Windows/Linux、不做 AU/AAX（使用者不用 Logic/Pro Tools）、不做行動平台。
- 不自建 NAM 訓練/capture 工具（用上游 Python 訓練器），只做**載入與播放**。
- MVP 期不做任意拓撲的訊號圖（平行分支、A/B path）——先做好線性鏈，拓撲擴充留給後期。
- 不做內建 looper/鼓機（願景清單，非 roadmap）。

---

## 4. 系統架構

### 4.1 執行緒模型與資料流

```
                  ┌────────────────────────────────────────────────┐
                  │  UI 執行緒（iced）                              │
                  │  參數操作、鏈編輯、preset 管理、meter/tuner 顯示 │
                  └───────┬───────────────────────────▲────────────┘
              參數變更/結構請求                   meter、tuner、xrun 統計
             （SPSC queue / atomics）           （triple buffer / SPSC）
                          │                           │
                  ┌───────▼───────────────────────────┴────────────┐
                  │  RT 音訊執行緒（cpal/CoreAudio callback）        │
                  │  參數平滑 → 線性效果鏈逐節點 process → limiter   │
                  │  ── 不配置記憶體、不上鎖、不做系統呼叫 ──        │
                  └───────▲───────────────────────────┬────────────┘
              新節點/新資產（Box 指標經 SPSC）     退役物件（送回釋放）
                          │                           │
                  ┌───────┴───────────────────────────▼────────────┐
                  │  資產工作執行緒（worker）                        │
                  │  解析 .nam / IR wav、建構 convolver、配置與釋放  │
                  └────────────────────────────────────────────────┘
                  （M6 加入：MIDI 執行緒 → 事件經 SPSC 進 RT）
```

核心不變式（詳細規則見 CLAUDE.md「Real-time audio rules」）：

1. **RT 執行緒永不阻塞**：不配置/釋放記憶體、不上鎖、不做 I/O、不打 log。
2. **所有跨執行緒溝通走 lock-free 結構**：`rtrb`（SPSC ring buffer）、`triple_buffer`、atomics、`arc-swap`。
3. **物件生命週期三段式**：worker 建構 → 原子交換進 RT → 舊物件送回 worker 釋放（「垃圾滑道」模式）。載入一顆新 IR 或 NAM 模型時，RT 執行緒只做一次指標交換。
4. **denormal 防護**：callback 進入時設 FTZ/DAZ；回授路徑（delay/reverb）不得殘留 denormal 造成 CPU 尖峰。

### 4.2 訊號鏈模型

MVP 是**可重排的線性鏈**（`Vec<EffectSlot>`），每個 slot 可 bypass：

```
in → gate → comp → drive → NAM amp → EQ → mod → delay → reverb → cab IR → limiter(常駐) → out
      └────────────── 順序可自由重排；tuner 從 input 分接，不在鏈上 ──────────────┘
```

- **Bypass** 帶短 crossfade（~10 ms）避免爆音。
- **Preset 切換策略**：同拓撲（只有參數不同）→ 參數層 morph；不同拓撲 → 換整條鏈 + 輸出端短淡出淡入（~30 ms）。delay/reverb 尾音保留（spillover）列為 M6 的 stretch goal。
- 平行分支、wet/dry 混合骨架在資料結構上預留（節點的輸入輸出聲道數已抽象化），但 MVP 不實作。

### 4.3 參數與 Preset 系統

- **穩定的 ParamId**：每個參數一個永不重用的 ID（`effect_kind:instance:param` 語意），為未來的 MIDI mapping 與 DAW automation 打底。
- **Normalized value**：UI/preset/MIDI 層一律 0.0–1.0，映射（線性/對數/skew）與顯示單位（dB、ms、Hz）定義在 `lh-core`。
- **平滑**：所有進入訊號路徑的參數經 per-sample 平滑（線性或一階低通，5–20 ms 視參數性質），杜絕 zipper noise。
- **Preset**：serde JSON、內含 `schema_version` 供未來遷移；外部資產（`.nam`、IR wav）以「路徑 + 內容雜湊」引用，並支援搜尋路徑重定位（換電腦不壞檔）。

### 4.4 Workspace 佈局

```
crates/
  lh-core      # ParamId、範圍映射、鏈模型、preset schema —— 無 I/O、無執行緒
  lh-dsp       # 手寫效果器；可離線測試、RT-safe
  lh-engine    # RT 圖執行器、節點生命週期、lock-free 管線
  lh-nam       # AmpModel trait + nam-rs 整合（為 FFI fallback 預留同一 trait）
  lh-io        # cpal 裝置管理、串流設定、延遲量測
  lh-assets    # worker 端載入：.nam、IR wav、convolver 建構
app/
  lion-heart   # 獨立 GUI 應用（iced）
```

鐵律：**`lh-*` 引擎 crate 永不依賴 GUI**。UI 可以整個換掉而不動聲音。

---

## 5. 技術選型（決策記錄）

### 5.1 語言：Rust（定案）

| 考量 | Rust | C++（JUCE） |
|---|---|---|
| 開發者熟悉度 | ✅ 較熟 | ❌ 需從頭學現代 C++ 與 CMake |
| RT 記憶體安全 | ✅ 所有權模型天然防 data race；RT 紀律可用型別系統輔助 | ⚠️ 全靠人工紀律 |
| 音訊生態 | ⚠️ 較年輕但已足夠：cpal、nam-rs、fft-convolver、nih-plug | ✅ 業界標準，範例最多 |
| 插件格式 | CLAP/VST3（nih-plug）；AU 缺席 | ✅ AU/VST3/AAX 全家桶 |
| 單人長期維護 | ✅ cargo 工具鏈、重構安全感 | ⚠️ 建置系統與 UB 的長期稅 |

**決策依據**：本專案不需要 AU（使用者不用 Logic）、不需要 AAX、獨立 App 優先、全程開源（GPL 相容不是問題）——C++/JUCE 的傳統優勢全數失效，而 Rust 路線曾經最大的缺口（NAM 推論核心是 C++）已由純 Rust 的 `nam-rs` 補上。單一維護者的生產力與 real-time 安全性成為決定性因素。

**否決方案**：C++/JUCE 全棧（優勢不適用）；Rust 核心 + JUCE 殼（兩套工具鏈的維護稅，等真正需要 JUCE 獨有能力再說）。逃生門：任何 C++ DSP 庫都可經 FFI 接入，`lh-nam` 的 `AmpModel` trait 就是為此預留的接縫。

### 5.2 音訊 I/O：cpal（起步）

- [cpal](https://github.com/RustAudio/cpal) 是 Rust 標準的跨平台音訊 I/O 抽象，macOS 走 CoreAudio。
- M0 用它完成 duplex 串流與延遲量測。若遇到抽象洩漏（buffer size 精細控制、aggregate device、多聲道路由），在 `lh-io` 內直接下沉到 `coreaudio-rs`——影響範圍被 crate 邊界鎖住。
- 研究關鍵字：CoreAudio audio workgroups（讓 DSP 輔助執行緒加入音訊 workgroup 的排程；單執行緒引擎用不到，先記錄）。

### 5.3 NAM 推論：nam-rs（主路線）

- [nam-rs](https://lib.rs/crates/nam-rs)（v0.3.x，MIT，2026-05 起活躍）：純 Rust、RT-safe（process 路徑零配置）、支援 WaveNet / LSTM / A2 模型，與官方實作有 1e-5 的 parity 測試，效能約 1.9 µs/sample（WaveNet）。
- **已知限制與對策：**
  - **取樣率鎖定**：`.nam` 模型綁定訓練取樣率（絕大多數為 48 kHz），rate 不符會「安靜地出錯聲」。⇒ **引擎正規取樣率定為 48 kHz**；裝置跑其他 rate 時在 I/O 邊界重採樣（`rubato`），或引導使用者把裝置設為 48 kHz。載入模型時校驗 rate 並在 UI 明示。
  - **單聲道**：吉他訊號本來就是 mono，不是問題。
  - **crate 年輕**（2026-05 首發）：鎖定 minor 版本、把 parity fixture 納入自家 CI 期望；fallback 路線是經 `cxx` FFI 綁 [NeuralAmpModelerCore](https://github.com/sdatkinson/NeuralAmpModelerCore)（C++/MIT），藏在同一個 `AmpModel` trait 後面，切換不動呼叫端（動用 fallback 需寫 ADR）。

### 5.4 IR 卷積：fft-convolver

- [fft-convolver](https://github.com/neodsp/fft-convolver)：HiFi-LoFi FFTConvolver 的純 Rust 移植。uniform partitioned FFT、**零延遲**（輸出與輸入 sample 對齊）、`init()` 後 process 路徑零配置。
- Cab IR 通常 20–200 ms（48 kHz 下約 1k–10k taps），uniform partition 完全夠用；`TwoStageFFTConvolver` 與 non-uniform partition 留給未來的卷積 reverb（M8+）。
- 周邊：IR wav 讀取用 `hound`（必要時 `symphonia`）；重採樣用 `rubato`（IR 檔 rate 與引擎不符時於載入期離線轉換）。

### 5.5 GUI：產品級，iced 為主要候選

使用者明確選擇**從一開始就投資產品級 UI**（乾淨現代視覺、自繪旋鈕、訊號鏈視覺化），因此 UI 是一等公民工作流，與 DSP 並行。

- 2026 年 Rust GUI 現況：`egui` 最快但偏 debug 風；`iced`（Elm 架構）較成熟、社群動能強；`vizia` 為音訊而生（CSS 式樣式、nih-plug 有現成 adapter）但成熟度仍被社群質疑。
- **策略**：M4 開頭做一次 timeboxed spike——同一畫面（自繪旋鈕 + 即時 meter + 60 fps）分別用 iced 與 vizia 實作，比較開發體驗、渲染負載與自繪控件成本，落選者寫進 ADR。**iced 為預設答案**，vizia 需在 spike 中證明自己。
- `egui` 保留給內部開發工具（debug HUD、引擎檢視器），不設限。
- 架構護欄：因為引擎與 UI 嚴格解耦（§4.4），此決策**可逆**——GUI 生態震盪最多損失 UI 層。
- 未來插件化時，iced/vizia/egui 皆有 nih-plug adapter，UI 技術可延用。

### 5.6 插件化路線（M7）

- [nih-plug](https://github.com/robbert-vdh/nih-plug)（ISC）：以 macro 匯出 **CLAP 與 VST3**，並有自帶的 standalone 匯出。屆時 Lion-Heart 的鏈引擎包成 plugin processor，UI 經 adapter 掛載。
- 授權注意：**VST3 匯出使 該建置產物 落入 GPLv3**（Steinberg SDK 授權使然）；CLAP 無此問題。本體維持 MIT OR Apache-2.0，發佈時 VST3 包以 GPLv3 隨附——對開源專案無痛。
- AU 明確不做（無 Logic 需求）；若未來需要，走 clap-wrapper 路線再評估。

### 5.7 依賴總表

| 用途 | Crate | 授權 | RT 路徑？ |
|---|---|---|---|
| 音訊 I/O | cpal | Apache-2.0 | 是（callback 宿主） |
| NAM 推論 | nam-rs | MIT | 是 |
| IR 卷積 | fft-convolver | MIT | 是 |
| FFT（自建模組用） | realfft / rustfft | MIT/Apache | 是 |
| SPSC ring | rtrb | MIT/Apache | 是 |
| 狀態發布 | triple_buffer、arc-swap | MPL-2.0 / MIT-Apache | 是 |
| RT 配置守衛 | assert_no_alloc | MIT（debug only） | debug |
| 重採樣 | rubato | MIT | 邊界/離線 |
| WAV 讀取 | hound | Apache-2.0 | 否（worker） |
| 序列化 | serde + serde_json | MIT/Apache | 否 |
| GUI | iced（候選）/ vizia | MIT | 否 |
| 基準測試 | criterion | MIT/Apache | dev |
| MIDI（M6） | midir 或 coremidi | MIT | 事件入列端 |
| 插件（M7） | nih-plug | ISC（VST3 產物 GPLv3） | 是 |

採用原則見 CLAUDE.md「Dependency policy」：進 RT 路徑的依賴，先讀其 process 路徑原始碼確認無配置/鎖。

### 5.8 授權策略

- 自有程式碼：**MIT OR Apache-2.0** 雙授權（Rust 生態慣例，對商用與 GPL 皆相容）。
- 依賴均為 permissive（見上表）；唯一 copyleft 觸點是未來 VST3 建置產物（GPLv3），已知且可接受。
- NAM capture / IR 檔是**使用者資產**，不隨 repo 散布，不涉授權。

---

## 6. DSP 模組規劃

原則：**音色自研、基建用現成**。每個模組列出初版演算法與研究關鍵字——這既是實作清單，也是學習大綱。共通介面為 `Effect` trait（block process / reset / apply params），一律可離線測試。

| 模組 | 初版演算法 | 研究關鍵字 / 資料 |
|---|---|---|
| **Noise gate** | envelope follower（attack/release）+ 遲滯雙門檻 + hold | hysteresis gate、high-gain 吉他 gate 行為 |
| **Compressor** | feed-forward、log-domain 增益計算、soft knee | Giannoulis et al.《Digital Dynamic Range Compressor Design — A Tutorial and Analysis》(JAES 2012) |
| **Drive / Boost** | 靜態 waveshaper（tanh、非對稱偏壓、diode-clipper 形）+ **4–8× oversampling**（half-band 多相）+ DC blocker + 前後 tilt EQ | aliasing 抑制、ADAA（Parker et al., DAFx-16）、Yeh 的 diode clipper ODE 研究 |
| **Tone stack** | Fender/Marshall 電路的解析離散化（bilinear transform） | Yeh & Smith《Discretization of the '59 Fender Bassman Tone Stack》(DAFx-06) |
| **NAM amp** | nam-rs（WaveNet/LSTM 推論），前後 gain staging | .nam 格式規格、WaveNet 膨脹卷積 |
| **EQ** | RBJ biquad cookbook 起步 → Zavalishin TPT/SVF | RBJ Audio EQ Cookbook；Zavalishin《The Art of VA Filter Design》（免費 PDF） |
| **調變系（chorus/flanger/phaser/tremolo）** | 共用 LFO 框架 + 分數延遲線（線性/allpass 插值）；phaser 用 allpass 級聯 | fractional delay、Dattorro《Effect Design Part 2》 |
| **Delay** | 環形緩衝 + 回授路徑濾波與軟飽和（analog 風味）；tempo sync 後置 | interpolated delay line、tape delay 特性 |
| **Reverb** | 8×8 FDN（Hadamard 回授矩陣、每線阻尼）；或 Dattorro plate 起步 | Jot & Chaigne (1991)、Dattorro《Effect Design Part 1》(1997)、Schlecht FDN 系列 |
| **Tuner** | 降取樣後時域法：YIN 或 MPM，UI 端顯示平滑 | de Cheveigné & Kawahara《YIN》(2002)；McLeod & Wyvill《A Smarter Way to Find Pitch》(2005) |
| **Limiter（常駐輸出）** | lookahead peak limiter（數 ms lookahead 可接受，計入延遲預算）或先做無 lookahead 硬保險 | brickwall limiter 設計 |
| **Metering** | peak + RMS，跨執行緒經 triple buffer | ballistics（積分時間常數） |

深水區（M8+ 研究線，不在 MVP）：WDF 白箱電路模擬（第一個題目：Tube Screamer 削波級）、卷積 reverb（non-uniform partition）、IR capture 工具鏈、自己的 WaveNet SIMD 推論實驗。

---

## 7. 里程碑

工作節奏為不定期爆發式，因此里程碑是**完成單位，不綁日期**；每個里程碑結束時都有「可以插琴彈的東西」，確保熱情回路。順序經過依賴排序，但單一里程碑內的順序自由。

### M0 出聲（First sound）
cpal duplex passthrough；裝置選擇（CLI 即可）；**loopback 實測 RTL 的量測工具**；xrun 計數器與報告。
**驗收：** 吉他 → Mac → 監聽出聲；RTL 實測數字寫進 `docs/latency.md`。

### M1 第一顆踏板（First pedal）
`Effect` trait 與參數系統 v0（ParamId、normalized、平滑）；noise gate + drive（oversampled waveshaper + tone）+ 簡版 delay；離線渲染測試框架 + null/golden 測試 + criterion 基準；debug 建置掛 `assert_no_alloc`。
**驗收：** 邊彈邊轉參數無爆音；測試綠燈；per-block 成本有基準數字。

### M2 音箱到位（The amp）
nam-rs 整合（`AmpModel` trait）；fft-convolver IR cab；輸入/輸出 gain staging 與 DC block；常駐輸出 limiter；48 kHz 正規化策略落地（含裝置 rate 檢查）。
**驗收：** 載入任一社群 `.nam` + IR，錄出一段自己滿意、可進 mix 的音色（與商業插件盲聽 A/B 不丟臉）。

### M3 鏈與記憶（Chain & memory）
線性鏈重排/bypass（含 crossfade）；preset JSON（schema_version、資產路徑+雜湊）；無爆音 preset 切換；app 設定持久化。
**驗收：** 建立 3 個常用 preset，任意切換無 click。

### M4 門面（The face）
GUI spike（iced vs vizia，timeboxed，落選寫 ADR）→ 產品級 UI 第一版：鏈視圖、自繪旋鈕、NAM/IR 瀏覽器、preset 瀏覽器、meter、tuner。
**驗收：** 不碰終端機可完成日常操作；60 fps；UI 執行緒不影響音訊（實測 xrun 不增）。

### M5 完整效果箱（Full pedalboard）
調變家族（chorus/flanger/phaser/tremolo，共用 LFO 框架）；FDN reverb；compressor；EQ 模組。
**驗收：** 個人常用的完整 live 音色鏈全部在箱內成立。

### M6 上台（On stage）
MIDI 腳控（midir/coremidi）：program change 切 preset、CC 映射、expression（wah/volume）；live view（大字體、腳邊可讀）；32-sample 效能硬化；（stretch）delay/reverb spillover。**此時添購 MIDI 腳控硬體。**
**驗收：** 一場排練全程用腳控完成，零 xrun。

### M7 插件與發行（Plugin & release）
nih-plug 匯出 CLAP + VST3（GPLv3 產物）；codesign + notarization；universal binary；GitHub Actions CI/release；v0.1 公開發布。
**驗收：** 陌生人能下載、通過 Gatekeeper、在 Ableton/Bitwig 掛載。

### M8+ 深水區（Deep water）
WDF 電路模擬研究、卷積 reverb、Windows（WASAPI）/Linux（PipeWire/JACK）移植、snapshot morphing、IR capture 工具。**進入條件：M7 完成且仍有熱情**——這是研究線，不是義務。

---

## 8. 風險與對策

| # | 風險 | 對策 |
|---|---|---|
| 1 | **單人 + 爆發式節奏 → 斷線後回不來** | 里程碑=完成單位且每站可玩；CLAUDE.md 與 ADR 讓 AI 協作快速重建上下文；白皮書即「與自己的合約」 |
| 2 | **RT 問題（xrun、click）難除錯** | 觀測先行：M0 就有 xrun 計數與延遲量測；debug HUD ring buffer；`assert_no_alloc`；效果全部可離線重現 |
| 3 | **Rust GUI 生態變動（vizia 停滯、iced breaking change）** | 引擎/GUI 嚴格解耦；GUI 決策可逆（§5.5）；最壞情況換 UI 不動 DSP |
| 4 | **nam-rs 年輕（2026-05 首發）** | 鎖版本；parity fixture 進 CI；`AmpModel` trait 後面備好 NeuralAmpModelerCore FFI fallback |
| 5 | **DSP 知識曲線陡** | 每模組附研究關鍵字（§6）；離線測試框架讓實驗不需接琴；先實作「教科書版」再調味 |
| 6 | **cpal 抽象洩漏（buffer 控制、裝置怪癖）** | 全部隔離在 `lh-io`；逃生門 coreaudio-rs；不讓 cpal 型別洩出 crate 邊界 |
| 7 | **延遲不達標** | 一切以 M0 量測為準；oversampling 只用在需要的節點；NAM 可選 lite/feather 模型；最後手段 SIMD 化熱點 |
| 8 | **Scope creep** | §3.4 非目標清單；新想法先進 backlog 不進 roadmap；拓撲擴充等 MVP 後 |
| 9 | **聽力/器材事故（高增益 + bug）** | 常駐輸出 limiter；啟動靜音 + 音量緩升；測試用 loopback 而非監聽全開 |

---

## 9. 成功標準

依序達成，每一條都可客觀驗證：

1. **錄音**：用 Lion-Heart 完成一首歌的全部吉他軌（M2–M3 後可達）。
2. **穩定**：48 kHz / 64 samples 連續一小時零 xrun（儀表為證）。
3. **延遲**：RTL 實測 ≤ 10 ms。
4. **上台**：帶 Mac + 介面（不帶地板）完成一場現場演出（M6 後）。
5. **社群**（弱指標）：v0.1 公開後出現第一個非本人的 issue 或 PR。

---

## 附錄 A：學習資源

**書與長文**
- Udo Zölzer（編）《DAFX: Digital Audio Effects》——效果器演算法百科
- Will Pirkle《Designing Audio Effect Plugins in C++》——概念與結構通用，語言無關
- Vadim Zavalishin《The Art of VA Filter Design》——濾波器聖經（免費 PDF）
- Julius O. Smith III 線上四部曲：<https://ccrma.stanford.edu/~jos/>
- Ross Bencina〈Real-time audio programming 101: time waits for nothing〉：<http://www.rossbencina.com/code/real-time-audio-programming-101-time-waits-for-nothing>——RT 鐵律的出處
- Jon Dattorro〈Effect Design〉Part 1/2（plate reverb 與調變效果經典）：<https://ccrma.stanford.edu/~dattorro/>

**論文（依模組）**
- 壓縮器：Giannoulis, Massberg & Reiss (JAES 2012)
- 破音抗混疊：Parker et al.〈ADAA〉(DAFx-16)；Yeh 系列（diode clipper）
- Tone stack：Yeh & Smith (DAFx-06)
- Reverb：Jot & Chaigne (1991)；Schlecht 的 FDN 系列
- Pitch：YIN (2002)；MPM (2005)

**社群與程式碼**
- musicdsp.org、KVR DSP forum、The Audio Programmer（Discord/YouTube）
- 可讀的開源專案：[NeuralAmpModelerPlugin](https://github.com/sdatkinson/NeuralAmpModelerPlugin)（C++）、[nih-plug 範例](https://github.com/robbert-vdh/nih-plug)、Guitarix（Linux 綜效前輩）、chowdsp 系列（現代 C++ DSP 品味極佳）

## 附錄 B：術語表

| 術語 | 意義 |
|---|---|
| RTL（round-trip latency） | 訊號從介面輸入到輸出的總延遲 |
| xrun / underrun | 音訊 callback 未在 deadline 內交出緩衝區造成的爆音 |
| NAM | Neural Amp Modeler——用神經網路 capture 真實音箱的開源生態 |
| IR | Impulse Response，此處指音箱體（cab）的脈衝響應 |
| Partitioned convolution | 將 IR 切塊做 FFT 卷積，兼顧低延遲與效率 |
| Oversampling | 在非線性處理前升取樣以抑制混疊（aliasing） |
| FDN | Feedback Delay Network，演算法式 reverb 的骨架 |
| WDF | Wave Digital Filters，白箱類比電路模擬方法 |
| SPSC | 單生產者單消費者的 lock-free 佇列 |
| Denormal | 極小浮點數觸發 CPU 慢路徑，回授路徑須防範 |
| Zipper noise | 參數階梯式跳變造成的雜音，以平滑消除 |
