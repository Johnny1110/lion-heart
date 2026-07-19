# PRD 006: Mod 家族擴編 — tremolo 大改 + 四顆新 pedal

狀態：**草案 → 開發中**
日期：2026-07-18
里程碑：M11
關聯：PRD 001（每 pedal 一張臉）、PRD 005（reverb 家族的 Ctl 路由模式）、白皮書 §4.2/§4.3

## 1. 背景與問題

使用者反映 **tremolo 開了跟沒開一樣**。根因有三，全是設計問題不是 bug：

1. **右聲道永遠反相（寫死的 auto-pan）**：L 谷底時 R 在峰頂，兩顆喇叭
   同時聽 L+R 幾乎恆定——房間裡聽到的是寬度晃動，不是 throb。真的音箱
   tremolo 是同相的。
2. **線性振幅深度律**：depth 0.5 只到 −6 dB；v2 遷移又把 mix 摺進
   depth（0.5×0.5=0.25 → −2.5 dB）。人耳聽的是 dB。
3. 只有 sine——沒有經典的 triangle（Fender 光電）與方波剁切。

同時 mod 家族只有四顆 pedal，缺整個「經典調變」譜系的另一半。

## 2. 目標

1. **Tremolo 大改**（faceplate: Rate / Depth / Wave / Spread）：
   - **dB-linear 深度律**：谷底 = −60 dB × depth（半深 −30 dB，全深
     近乎靜音）。峰頂永遠是 unity——只挖谷，不整體變小聲。
   - **Wave**：sine / triangle / **chop**（slew 限速的方波，~1 ms 邊緣
     防 click——直升機剁切）。
   - **Spread** 0..1 = 右聲道 LFO 相位 0..180°。**預設 0（同相）**：
     這才是「開了就聽得到」的修復本體；轉滿變 hard ping-pong
     （兩邊輪流 gate；dB 律是凸的，總和不守恆——這是特性不是缺陷）。
   - `rate`/`depth` key 與 v2 摺疊遷移完全相容（位置 0/1 不動，新鈕
     append）。舊 preset 載入後 tremolo 會**變明顯**——這正是需求。
2. **四顆新 pedal**（registry append-only，v2 遷移的 `MOD_PEDALS`
   index 映射只涵蓋前四顆，不受影響）：
   - **vibrato**（Rate/Depth）：真音高顫音——wet-only 掃動延遲讀頭，
     **左右聲道相位一致**（音高彎折是一個事件，不是加寬工具）。
   - **harmonic**（Rate/Depth）：brownface 諧波顫音——700 Hz 互補分頻，
     低頻帶與高頻帶**反相**增益調變；音色在晃、音量不晃。depth 0
     bit-exact 直通。
   - **rotary**（Speed/Depth/Balance）：小 Leslie——800 Hz 分頻，
     號角與鼓各自的 doppler + AM + pan；**各自的慣性**（號角 ~0.9 s、
     鼓 ~3.2 s）——切 slow⇄fast 的 spin-up/down 就是這台效果器的靈魂。
     Balance 是 drum⇄horn 等功率交叉。切進 rotary 時轉子從 slow 起步，
     收到 fast 值後滑上去（到位即 spin-up）。
   - **univibe**（Rate/Depth）：光電 vibe——四級 allpass 在**錯開的**
     轉角（78/210/620/1750 Hz）被同一顆偏斜（燈泡式）LFO 掃動，
     50/50 固定混合。跟 phaser（同轉角四級 + feedback）判然兩物。
3. **共用 param→Ctl 路由表**（照 reverb 家族的做法）——faceplate 不再
   共享旋鈕位置。

## 3. 非目標

- 無 stereo 輸入處理路徑改動（rotary 進 cabinet 前 mono-sum，如實體）。
- 無 tap tempo on rate（delay 已有；mod rate 用手轉）。
- univibe 不做 expression 踏板掃速（v1 無 expression 架構）。
- 不動 chorus/flanger/phaser 的既有音色與 param。

## 4. 驗收標準

1. `cargo test`：registry 前四顆釘 `MOD_PEDALS`；tremolo 同相深挖
   （L、R、L+R 皆 pump，預設深度就明顯）、spread=1 包絡反相關、chop
   有 gate 且無 click 級階躍；vibrato 載波散開但 RMS 不變、L==R
   bit 級一致；harmonic 兩帶反相關、depth 0 bit-exact；rotary 慣性
   加速可測；univibe ≠ phaser;全家族有限/有界/靜默/多取樣率不變量。
2. `cargo bench`：八顆 pedal 各一列，最重 < 0.25 % deadline。
3. 舊 v2/v3/v4 preset 載入不變（tremolo 更響是規格內變更）。
4. 耳朵驗收（使用者）：tremolo 開下去要「一聽就在」；rotary 切速聽
   spin-up；harmonic 配 clean 聽 brownface 浪；univibe 配 drive 聽
   Machine Gun。
