# PRD 010: Spillover — 尾音跨 preset / 移除存活

狀態：**草案 → 開發中**
日期：2026-07-19
里程碑：M13
關聯：PRD 002（動態鏈：install/remove/retire）、白皮書 §7 M6 stretch
（「delay/reverb spillover」）、白皮書 §3.3（常駐輸出 limiter）、
§4.2（無爆音）

## 1. 背景與決策

現在切 preset 有兩種殺尾音的方式：同 family 生還者被灌新參數（delay
time 一改，殘響變滑音噪訊）；被移除的 slot 在 4 ms master fade 底被
硬砍。專業機（Strymon、Helix）的行為是：**舊尾音自然響完，新 preset
立刻可彈**。白皮書 M6 把它列為 stretch，這裡落地。

拍板：引擎長出 **spill lane**（4 條 stereo，預先配置），掛在
**master fade 之後、輸出級之前**——所以換 preset 的 order fade 殺不到
它，safety limiter/EQ 仍然罩著它。這是三個 feature 裡唯一動 RT 生命
週期的（retire chute 之外多一條 spill 出口），需正式 ADR。

## 2. 引擎（RT）

- **`EngineMsg::SpillSlot { index }`**：跟 `RemoveSlot` 差在佔用者不進
  retire chute，而是**立即**移進一條空 lane（音訊執行緒上純指標搬移，
  零配置）。slot 立刻從 `slots[index]` 取出 → 主鏈 `order` 即使還列著
  該 index 也會 skip（`None`），lane 立刻接手播尾音（無 fade dip）。
- **Lane 處理**（每 chunk）：餵靜音輸入跑 lane 的 effect（尾音 = wet
  殘響，dry=0）→ 乘 lane gain → 加總進 bus → 追蹤 peak/計時。
- **出口條件**：
  1. **靜音逐出**：輸出 peak < −80 dBFS 連續 ~250 ms → 推進既有 retire
     chute，釋放 lane。
  2. **強制衰減**：進駐超過 ~8 s → 每 sample 乘 −12 dB/s 的 gain
     ramp。**必要**：tape/vintage delay feedback ≥ 1 的 bounded 自激
     振盪不餵輸入也永不停；forced decay 壓到 −80 dB 後靜音逐出接手。
     gain < 1e-7 直接歸零（denormal 防護，RT 規則 7）。
  3. **Lane 滿了**（4 條全佔）再來一個 spill → 淘汰**最舊**的（硬砍，
     其尾音最衰減、最不明顯），讓位給新的。4 條對「快速來回切兩個
     delay-heavy preset」已很寬裕；硬砍是罕見邊界。
- **成本**：最壞 4 lane × 最貴 reverb ≈ +18 µs / 64-frame block，對
  1333 µs deadline 是 ~1.4 %。`assert_no_alloc` 必須維持綠燈（指標
  搬移與 scratch 皆預配置）。

## 3. 控制側

- **`Effect::tail_seconds()`**：trait 加一個預設 0 的方法；delay/reverb
  覆寫回傳保守上限（靜態提示，不是精確尾長——精確長度由引擎的靜音
  偵測決定）。install 時快取進 `SlotShadow.tail_secs`，讓控制側不需
  觸碰音訊執行緒的 effect 就能決定「該 spill 還是硬砍」。
- **`ChainHandle::spill_slot(handle)`**：從鏈移除 slot（同 remove 的
  控制側簿記）但送 `SpillSlot` 而非 `RemoveSlot`。
- **preset 載入 reconcile**：`apply_preset_chain` 多收一個 `spillover:
  bool`。Pass 2 釋放未認領的 slot 時，若 `spillover && tail_secs > 0`
  → spill；否則照舊 remove。**有尾巴的 slot 一律 spill + 裝全新實例**
  （新實例乾淨起步，沒有 time-glide 假音）——所以生還者認領不再套用
  在有尾家族上（避免「同 delay 被灌新參數變噪」）。
- **板面編輯 remove**：session 的 remove 包裝依 `spillover && tail>0`
  選 spill/remove——拔掉正在響的 reverb，尾音響完才消失。
- **設定**：`AppConfig.spillover: bool`（config.json，**預設 on**）。
  關閉 = 控制側一律走 remove（引擎 lane 閒置）。REPL `spillover
  on/off`。

## 4. 非目標

- 每 slot 獨立 spillover 開關（全域一面旗）。
- lane 滿時的淡出讓位（v1 硬砍最舊；淡出要額外過渡 slot）。
- plugin spillover（host 有自己的尾音/凍結；standalone first）。
- spill 尾音存進 preset（尾音是瞬態，不是狀態）。

## 5. 驗收標準

1. `cargo test`（離線引擎，無裝置）：preset A 打 impulse 進 delay →
   切到不含該 delay 的 preset B → 輸出仍含 A 的 echo 間距且衰減中；
   靜音逐出釋放 lane；lane 耗盡淘汰最舊；tape 自激振盪被 forced decay
   蓋掉（有界、最終靜音）；全程無 NaN；`spillover off` 走硬砍（尾音
   立即斷）。
2. `assert_no_alloc`：debug 建置跑 spill 路徑不 abort。
3. `cargo bench`：`spillover_worst`（4 lane 滿載 reverb）記錄成本。
4. 耳朵驗收（使用者）：delay/reverb-heavy preset A 彈著切 B，尾音自然
   響完、新音色立刻可彈；板面拔掉響著的 reverb，尾音續響；快速來回切
   兩個空間 preset 不爆音；`spillover off` 尾音立即斷（對照）。
