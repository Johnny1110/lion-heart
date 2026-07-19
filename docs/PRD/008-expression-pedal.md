# PRD 008: Expression 全通路 — 手動 wah、CC 塑形、MIDI learn

狀態：**草案 → 開發中**
日期：2026-07-19
里程碑：M13
關聯：PRD 007（filter 家族；其非目標「腳踏 wah 等 expression 架構」即本篇）、
白皮書 §7 M6（「CC 映射、expression（wah/volume）」的未竟項）

## 1. 背景與決策

M6 交付了 PC 切 preset 與 raw CC 映射（`midi.json` 的 `cc` 表 →
`slot.param`，0..127 線性打滿 0..1），但白皮書 M6 點名的
**expression（wah/volume）** 一直是斷的：

1. **沒有值得踩的目標**——autowah 是自動的；手動 wah 在 PRD 007
   被明確推遲到「expression 架構」到位。
2. **CC 只有裸線性**——volume 踏板要 audio taper、反向踏板要翻轉、
   只想掃半程的要範圍，全都做不到。
3. **綁定要手改 JSON**——現場排練不可能開編輯器。
4. **值跳躍**——preset 切換後實體踏板位置 ≠ 參數值，一動就跳。

拍板：四件事一起做完，expression 才算通。全部落在控制側與 DSP 新
pedal，**引擎零改動**（`SetParam` 平滑層本來就是為此存在的）。

## 2. 手動 wah（filter 家族第二顆 pedal）

**Faceplate（4 鈕）**：`pos`（0–1，踏板位置——expression 的落點，
平滑 25 ms 吸掉 7-bit CC 階梯）/ `q`（1.5–12 log，預設 6——人聲
共振比 autowah 預設尖）/ `mode`（lowpass/bandpass/highpass，預設
lowpass——經典 wah 峰下有身體）/ `mix`（預設 1.0）。

**訊號路徑**：`pos` 幾何映射 **350 Hz → 2.2 kHz**（Crybaby 區間，
比 autowah 的 180–2400 窄——手動 wah 是人聲母音，不是 funk 濾波器）
→ 與 autowah 共用的 Chamberlin SVF（每聲道獨立、band 軟剪自限）。
無 envelope follower、無 `direction`（反向踏板交給 mapping 的
min/max 翻轉）。

**結構**：`filter.rs` → `filter/` 目錄（mod.rs 共用引擎 + 每 pedal
一檔），照 delay 家族的 `Ctl` 路由表模式。追加 pedal 是 append-only：
無 preset schema 升版、plugin 參數自動展開（`filter_wah_*`，
pre-v0.1 id 追加，重跑 clap-validator）、theme 加 wah 專屬 livery
且 filter 家族進 distinct-livery 釘測。

## 3. CC 塑形（lh-midi）

`cc` 表的值升級為**字串或物件**（serde untagged，舊檔原樣可讀）：

```json
"cc": {
  "11": { "target": "filter.pos", "min": 0.0, "max": 1.0,
          "curve": "linear", "pickup": true },
  "7":  { "target": "amp.output", "curve": "audio" },
  "80": "gate"
}
```

- `min`/`max`（預設 0/1）：normalized 落點範圍；**min > max = 反向**。
- `curve`：`linear`（預設）| `audio`（x²——volume 踏板的對數手感）。
- `pickup`：soft-takeover，見 §4。
- 塑形只作用於連續參數；bare-slot bypass 目標維持 value ≥ 64 語意。

## 4. Soft-takeover（pickup）

控制側（session）為每個 `pickup: true` 的 mapping 記「咬合」狀態：

- **失同步事件**：preset 載入、該 slot 換 pedal、GUI 旋鈕手動改了
  該參數 → 解除咬合。
- **重新咬合**：塑形後的 CC 值**掃過**目前參數值（前後兩筆事件跨越
  它）或進入 ±0.02 內 → 咬合，之後直通。
- 未咬合期間 CC 靜默（不跳值）。狀態純控制側，不碰音訊執行緒。

## 5. MIDI learn（GUI）

- **右鍵旋鈕** → learn 模式：下一個進來的 CC（尊重 channel 濾波）
  綁定該 `slot.param`，寫回 `~/.lion-heart/midi.json`（保留
  input/channel/pc_presets），解除武裝並回報；同一 CC 原有綁定被
  覆蓋並回報被換掉的目標。
- learn 中：參數面板顯示 banner（目標、取消鈕）；再右鍵同一顆 =
  取消，右鍵別顆 = 改武裝。
- 已綁定的旋鈕戴小徽章（accent 圓點 + CC 號）；banner 提供「清除
  綁定」。
- REPL 對應：`learn <slot.param>` / `unlearn <slot.param>`。
- learn 寫入的條目用字串簡式；塑形（curve/pickup）手改 JSON 加上
  ——GUI 塑形編輯器是後話，不在本篇。

## 6. 非目標

- 14-bit CC（7-bit + 25 ms 平滑已無階梯感；有需求再開）。
- Plugin 內的 MIDI learn/pickup——host automation 是那邊的正解。
- GUI 塑形編輯器（min/max/curve 仍手改 JSON）。
- LFO wah / S&H / formant——家族的下一批。

## 7. 驗收標準

1. `cargo test`：wah 峰值追 `pos`（低/高位置增益比）、pos 硬跳無爆音
   （declick 界內）、家族不變量（有界、mix 0 bit-exact、多取樣率、
   全鈕掃掠）；lh-midi 新舊格式 round-trip、塑形數學（min/max/
   invert/audio）、pickup 咬合邏輯；theme filter 家族 livery 相異；
   registry/plugin pin 測試隨 2-pedal 家族更新。
2. `cargo bench`：`filter_wah` 與 `filter_autowah` 同量級（< 0.15 %
   deadline）。
3. 舊 `midi.json`（純字串 cc 表）原樣可讀、行為不變。
4. 耳朵驗收（使用者）：expression 踏板掃 wah 順滑無階梯、volume
   曲線 audio taper 手感對、preset 切換後踏板不跳值（pickup）、
   GUI 右鍵 learn 一次成功、徽章正確。
