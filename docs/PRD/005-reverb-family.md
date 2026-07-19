# PRD 005: Reverb 家族化 — 十二台機器

狀態：**草案 → 開發中**
日期：2026-07-18
里程碑：M10
關聯：PRD 004（delay 家族）、PRD 001（每 pedal 一張臉）、白皮書 §4.2（無爆音）、§4.3（平滑層）、§7（RT 規則）

## 1. 背景與問題

`reverb` 目前是**單一 pedal 家族**：M5 的 8 線 Householder FDN，只有
`decay / tone / predelay / mix` 四顆旋鈕。使用者要的是 BigSky 等級的
多機種 reverb——hall、spring、shimmer 這些不是「同一台機器轉旋鈕」能到的
音色，是**結構不同**的演算法。

M9 已經把「一個 slot 掛一個家族、每 pedal 自己一張臉、shadow 記憶、
preset 分存、GUI/plugin 自動展開」整套機制跑通（delay 三 pedal）；reverb
只是同一模式的更大實例。

## 2. 目標

**Reverb 升級成十二 pedal 家族**（家族 key 仍為 `reverb`，順序 append-only；
靈感對應 BigSky 的十二台機）。共用鈕:**Decay / Predelay / Mix / Tone /
Mod**（= BigSky 面板的五顆),加上每台機至多兩顆 signature 旋鈕:

| 機器 | 結構 | signature 旋鈕 | 音色目標 |
| --- | --- | --- | --- |
| **hall** | FDN tank | Size、Low End | M5 原音色（遷移目標）;音樂廳到體育館 |
| **room** | FDN tank（小） | Size、Diffusion | 錄音室殘響到夜店包廂;wet 比 hall 早到 |
| **plate** | FDN tank（密） | Size、Low End | 快速堆密、最亮 tone 範圍、無空間感提示 |
| **spring** | 色散鏈 + 小 tank | Dwell、Springs (1–3) | 彈簧「boing」;dwell 推進去會咬 |
| **swell** | FDN + 起音包絡 | Rise、Mode | 每個音頭 wet 從零漸入;mode 可連 dry 一起 |
| **bloom** | 再生擴散圈 + FDN | Feedback、Length | 攻擊被抹開、殘響「開花」慢慢長大 |
| **cloud** | 最大 FDN | Haze | 巨大、暗、霧;haze 追加第 3/4 級擴散 |
| **chorale** | FDN + 母音共振 | Vowel、Intensity | 尾音像人聲合唱 A→E→I→O→U 漸變 |
| **shimmer** | FDN + 移調回授 | Amount、Interval | 每圈拉高一個八度/五度的天梯感 |
| **magneto** | 多磁頭 echo → FDN | Spacing、Repeats、Heads (1–4) | 鼓式磁帶 echo 拖著殘響尾巴 |
| **nonlinear** | 無回授多 tap 爆發 | Shape (gate/reverse/swoosh) | 違反物理的包絡;窗結束聲音就結束 |
| **reflections** | 早期反射 tap 表 | Size、Shape (studio/chamber/dome) | 只有房間、沒有尾巴;把音箱搬進空間 |

規格細節：

1. **`tone` 保留 v4 的 key/單位（Hz、log）**——它就是阻尼轉角。hall 的
   `decay/tone/predelay/mix` 與舊 `reverb` pedal **同 key 同範圍同預設**，
   遷移後舊檔案音色不變。
2. **Low End**（hall/plate）0..1、0.5 中性：以下=迴圈內低頻衰減加速
   （low RT 縮短，只做損耗、絕對穩定）;以上=**迴圈外**輸入 low shelf
   增益（增厚不碰穩定性）。
3. **Mod**：一顆 LFO 以相位旋轉分配到八條線（每 sample 一組 sin/cos，
   不是八次 sin），深度 0..1、速率是各機固定音色。
4. **有界性**：shimmer 回授先過 `tanh` 軟剪（同 delay 自振盪設計）;
   bloom 迴圈增益上限 0.85;nonlinear/reflections 根本沒有回授。
   任何旋鈕組合永不 NaN/失控（白皮書 §7）。
5. **Preset schema v4 → v5**：舊 `reverb` pedal 更名 `hall`
   （`REVERB_PEDALS` 釘家族順序）;稀疏 slot（沒記 pedal）也落在 hall。
6. GUI/plugin/engine/session **零程式碼變更**（多 pedal 機制自動涵蓋）;
   plugin param id 換代（`reverb_hall_decay`…）——pre-v0.1 break，
   重跑 clap-validator。

## 3. 非目標

- 不是 BigSky 克隆——是同一張機器清單的 Lion-Heart 詮釋;參數對應到
  我們的旋鈕典範（例如 reflections 沒有 listener position）。
- 無 freeze/hold、無 infinite sustain 腳踏行為（等腳控設計時一起做）。
- 無 pre/post-delay ducking、無 dual-machine 並聯（BigSky MX 領域）。
- Chorale 不做完整 vocoder——兩顆共振帶通的母音暗示。

## 4. 使用者故事

- 我把 reverb 切到 spring 配 crunch 音色，聽到彈簧的「啪-boing」;
  dwell 轉大彈簧開始咬。
- 我切 shimmer 撥 +octave、amount 拉滿彈一個和弦，尾巴一路往上爬成
  管風琴雲，放著不管也不會炸。
- 我切 nonlinear 選 gate 打悶音 riff，殘響像 80 年代鼓組一樣被切掉。
- 我載入去年存的 v4 preset，reverb 還是原來那顆 hall 的聲音。

## 5. 驗收標準

1. `cargo test`：家族 registry 釘住 `REVERB_PEDALS`;hall 保持 M5 行為
   （衰減單調、decay 拉伸、predelay 位移、立體聲去相關、mix 0 bit-exact）;
   十二台各自的性格測試（swell 漸入、bloom 再生、shimmer 八度、magneto
   磁頭落點、nonlinear gate 切斷/reverse 上升、reflections 無尾、chorale
   母音共振、spring 諧波、plate 高頻餘響、room 早到、lowend 雙向、mod 擺動）;
   全家族共通不變量（有限、有界、靜默進出、多取樣率/block、切 pedal 不炸、
   每顆旋鈕掃掠不炸）。
2. `cargo bench`：每台機一列;最重的 voice < 1 % deadline。
3. 舊 v3/v4 preset 載入：reverb slot 落在 hall、數值原封、聲音不變。
4. 耳朵驗收（使用者，Mac + 真琴）：十二台輪一遍;spring/shimmer/
   nonlinear/magneto 重點聽;preset 存讀含非 hall 機種。
