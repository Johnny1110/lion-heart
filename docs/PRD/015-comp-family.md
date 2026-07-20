# PRD 015: Comp 家族 — VCA / Opto / FET 三種壓縮拓撲

狀態：**草案（待開發）**
日期：2026-07-20
里程碑：M18（2026-07-20 路線圖第 5 項）
關聯：PRD 001（per-pedal 參數）、ADR 007（delay 單→家族 + 遷移前例）、
ADR 014（eq 家族化前例）、白皮書 §4.2

## 1. 背景與決策

現況 comp 是單踏板數位 VCA（`dynamics::comp`，DESC/FAMILY key 均為
`comp`，threshold/ratio/attack/release/makeup）。使用者要更多壓縮個性——
三種經典拓撲各有招牌手感。

拍板：**comp 單→三踏板家族**（delay v3→v4 的路線，含 schema 升版）：

1. **改名 + schema v7**：現有 VCA 踏板 key `comp` → **`vca`**（三拓撲
   `vca/opto/fet` 是真實器材分類，`comp/opto/fet` 會混淆家族名與踏板
   名）。`migrate_v6_comp_pedal` 把舊 preset 每個 comp slot 的 pedal-values
   map key `comp`→`vca`（同 `migrate_v3_delay_pedal` 改 delay→digital），
   `PRESET_SCHEMA_VERSION` 6→7，`COMP_PEDALS` pin 註冊順序。無 pedal 欄的
   舊 slot 本來就 fallback 到 index 0 = vca，值靠這條遷移原味保留。
2. **三踏板**（per-pedal `Ctl` 表，delay/filter 模式）：
   - **vca**：現有數位壓縮，逐字保留（threshold/ratio/attack/release/
     makeup）+ 下述兩顆共用鈕。
   - **opto**（Teletronix LA-2A 式）：**program-dependent release**——雙
     時常數（快段快回、久壓慢回），軟膝，固定慢 attack；面板精簡
     `peak_reduction`(=threshold 反向)/`gain`(makeup) + 共用鈕（無
     attack/ratio 鈕，光耦本質決定）。音色：黏、慢、圓。
   - **fet**（UREI 1176 式）：極快 FET attack（微秒級）、ratio 含
     **all-buttons-in**（stepped 4:1/8:1/12:1/20:1/all），makeup；快到能
     當 transient 塑形。音色：快、脆、attack 咬。
3. **兩顆家族共用新鈕**（加到三踏板 faceplate，additive）：
   - `blend`：0..1 平行壓縮乾/濕混音（parallel comp，New York 手法）。
   - `sc_hpf`：sidechain 高通 20–300 Hz——低頻不觸發壓縮（低音不抽吸）。
4. **DEFAULT_CHAIN 不變**（comp 仍在 gate/filter **之後**、drive 之前，
   位置不動）；plugin 自動展開多踏板參數 + `comp_pedal` 選擇器。

## 2. 規格

三拓撲共用一個偵測器結構（peak/RMS envelope follower + sidechain HPF），
`VoiceDef`/`Ctl` 表切換時常數法則與面板，delay 家族同款單引擎 match。
`blend` 平行混音在踏板尾端（壓縮訊號 × blend + 乾 × (1−blend)）；vca 的
既有增益法則不動（回歸測試釘住舊 preset 聽感）。

**Plugin id break（pre-v0.1）**：comp 由單→多踏板，既有 `comp_threshold`
→ `comp_vca_threshold`（eq 家族化同款 rename），新增 `comp_opto_*`/
`comp_fet_*` + `comp_pedal`。重跑 clap-validator。

**Livery**：opto/fet 各自 signature 色（vca 保留家族藍），納入
distinct-livery pin（家族清單 +comp）。

## 3. 非目標

- 真實電路級（WDF）壓縮建模——這是行為級手寫 DSP（拓撲手感，非電路）。
- 多頻段壓縮 / de-esser / 上行壓縮（upward）。
- sidechain 外部訊號輸入（sc_hpf 是內部 sidechain 濾波，非外接）。

## 4. 驗收標準

1. `cargo test`：三拓撲時常數相異（opto 雙時常數 program-dependent、fet
   快 attack、vca 保留現值）、blend 0 = 位元透明乾訊、blend 1 = 全壓、
   sc_hpf 讓低頻不觸發（低頻大訊號壓縮量 < 中頻）、all-buttons-in ratio
   極端有界、registry 一致（COMP_PEDALS pin、controls 對齊）、schema v7
   遷移（舊 v6 comp 值→vca 逐字保留，聽感不變）、多 rate/block。
2. `cargo bench`：三踏板各 < 0.15 % deadline。
3. 舊 preset：v6 載入後 comp slot = vca、threshold 等值不變、聽感一致。
4. 耳朵驗收（使用者）：opto 黏慢圓的 clean comp、fet all-in 的 pumping、
   vca 透明整平；blend 平行壓縮保 transient；sc_hpf 開後低音不抽吸；
   舊 preset 的壓縮聽起來沒變。
