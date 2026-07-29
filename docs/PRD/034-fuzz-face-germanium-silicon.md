# PRD 034: `fuzz-face` 加 Germanium / Silicon 型號選擇（Phase 05 第三項）

狀態：**已實作（2026-07-29）— 待使用者耳朵驗收**
日期：2026-07-29
里程碑：Tone Revolution · Phase 05（`docs/tone_revolution/phase/05-fuzz-transistor-family.md` §2.2）
關聯：**ADR 035 §4**（為什麼維持 behavioral、NDK 為何列未來研究）、
PRD 024 / ADR 031（ADAA，本顆的硬截止正是它存在的理由）
新增 ADR：**無**（ADR 035 §4 已載明決策）

## 1. 背景與決策

計畫 §2.2 對 Fuzz Face 的拍板是「**維持 behavioral**，選配小幅精修」。維持的理由在
**ADR 035 §4**：參考實作的 NDK 係數由**私有工具**產生，沒有可移植的公開產生器，
自建是一整套框架的工作量，而且與本 Phase 其他兩顆共用不了任何東西。

本 PRD 做的是那個「選配」，而且挑了最有內容的一項：**Germanium / Silicon 型號選擇**。
理由不是「多一個選項」——1968 年起 Dallas Arbiter 把 NKT275 鍺換成 BC108/BC183 矽，
**那是兩顆不同的踏板**，而且是彈過的人都同意的那種不同。既有的 behavioral 模型
（tanh 上、硬夾下、bias offset、ratio gate）恰好每一項都能從器件推出差異來。

## 2. 規格

### 2.1 每一項差異都從器件推出來

`Voice` 表的六個欄位，一項一個理由：

| 欄位 | 鍺 | 矽 | 器件上的理由 |
| ---- | -- | -- | ------------ |
| `gain_db` | 20 → 55 | 26 → 64 | 矽的 β 是鍺的 3–5 倍，掃程兩端一起上移 |
| `knee_pos` / `knee_neg` | 0.9 / 0.5 | 0.75 / 0.68 | 矽的 `Vbe` 可預測、漏電可忽略 → Q1 偏在電阻算出來的地方 → **兩個門檻靠攏**；同時兩者都低一些 = 更硬、更方 |
| `pre_bias` | 0.02 | 0.006 | 同上：偏壓靠近中點 → duty cycle 拉直 → **偶次諧波變薄** |
| `gate_frac` | 0.25 | **0（關閉）** | velcro splutter 是 **blocking distortion**，源自鍺的漏接面與 0.2 V 壓降。矽 Fuzz Face 順順地延音，出了名地**不做這件事** |
| `dark_hz` | 5.5 kHz | 9 kHz | BC108 沒有一絲 woolly |
| `makeup` | 0.13 | 0.125 | 各自校準，切換型號是換性格不是換音量 |

`gate_frac = 0` 是**明確關閉**（`gate_frac <= 0.0 || …`），不是「門檻低到不會觸發」——
後者在第一個 sample 仍會有一段淡入。

### 2.2 面板與相容性

面板 2 顆變 3 顆：**Fuzz / Type / Volume**。`Type` 走 `Ctl::Shape`（stepped，
不進平滑器），與 `ts-wdf` 的 Diode、`waveshaper` 的 Shape 同一條路。

**追加是安全的**，這點值得寫下來，因為它決定了「能不能改既有踏板的面板」：

- preset 以**參數 key** 存值（`BTreeMap<String, f32>`），舊檔沒有 `type` → 取預設
  **0 = Germanium** = 完全既有的行為；
- plugin 的參數 id 是 `{slot}_{pedal_key}_{param_key}`（**名稱**，不是位置），所以既有
  id 一個都沒動，host 的 automation 不會錯位。

`VOICES` 是 **append-only**（preset 存的是索引）。

### 2.3 既有行為不動

索引 0 的六個值**逐字等於**改動前的常數，所以 `fuzz_face_gates_the_decay`、
`fuzz_face_is_strongly_asymmetric`、`fuzz_face_has_no_clean_floor`、
`fuzz_face_cleans_up_at_low_input` 四條既有 character 測試**一條都沒有重新 pin**。
這是刻意的：型號選擇是**加一顆踏板**，不是重調一顆。

## 3. 驗收標準與實測

### 3.1 `cargo test`（新增 3 條，既有 4 條不動）

| 測試 | 標準 | 實測 |
| ---- | ---- | ---- |
| `fuzz_face_silicon_is_squarer_and_brighter_than_germanium` | 矽的二次諧波 < 鍺的 0.6 倍；矽的九次 > 鍺的 2 倍 | 通過（h2 **0.039 vs 0.102**、h9 **0.095 vs 0.031**） |
| `only_the_germanium_fuzz_face_gates` | 矽的 tail/body > 鍺的 20 倍 | 通過（**0.188 vs 0.000**） |
| `both_fuzz_face_voices_sit_at_the_same_level` | 兩者相差 < 1.5 dB | 通過（−0.09 vs −0.01 dB） |

鍺的 tail/body 量到 **0.0000**——它把尾巴切到量不出來，而矽一路延音。這是兩者最聽得
出來的差別，也是這三條裡最有說服力的一條。

### 3.2 `cargo bench`

`drive_fuzz-face_4x_oversampled` ~19.4 µs（÷screamer 0.54）——與改動前同量級。
每 sample 多的是兩個從 `Voice` 讀出的常數與一次比較；`germ_clip` 由 const 常數改成
closure 捕獲，`Adaa1::process` 本來就吃 `impl Fn`，內聯後代碼形狀不變。

### 3.3 耳朵（**待使用者驗收**）

- 切 Type 聽兩件事：**尾巴**（鍺 velcro 切斷 vs 矽順順延音）與**上緣**（woolly vs 刺）。
- 兩者音量應該一致；若有落差回報，`makeup` 是一個數字的事。
- 鍺 = Hendrix 早期 / Gilmour；矽 = 1969 之後那個更兇更亮的聲音。

## 4. 非目標

- **不做 NDK**（ADR 035 §4）。
- 不做電晶體挑選（hFE 匹配、bias trim pot）——真實 fuzz 玩家的樂趣，但那是 Phase 08
  自研平台的題目。
- 不加 tone 控制（真實踏板沒有）。

## 5. 已知取捨

- **仍是 behavioral**：`knee_pos` / `knee_neg` / `pre_bias` 是**擬合出來的形狀參數**，
  不是從電路解出來的。與同 Phase 的 `rangemaster`（真器件模型）並列時這一點很明顯，
  也是刻意留著的對照——ADR 035 §4 說明為什麼。
- 兩組 voicing 是**代表值**，不是某兩顆實機的量測。
- 型號切換會 `reset()` 削波器與 gate 狀態（換一對電晶體是換踏板，不是轉旋鈕），
  所以彈奏中切換有一個短暫斷點——與家族切換踏板的行為一致。

## 6. 產出

- `crates/lh-dsp/src/drive/fuzz_face.rs`：`Voice` / `VOICES` 表、`Type` 面板參數、
  `set_shape`
- `crates/lh-dsp/src/drive/mod.rs`：`fuzz-face` 的 controls 加 `Ctl::Shape`、
  faceplate pin 更新、3 條新測試
