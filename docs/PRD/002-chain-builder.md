# PRD 002: 自由效果鏈編輯器

狀態：**草案 → 開發中**
日期：2026-07-17
里程碑：M8（與 001、003 同組）
關聯：白皮書 §4.2（訊號鏈模型）、§4.3（instance 語意）、PRD 001

## 1. 背景與問題

目前的鏈是固定十格（gate→comp→drive→amp→eq→mod→delay→reverb→cab→
limiter）：只能重排與 bypass，不能增刪、不能同 family 多顆。想要
「comp → drive → drive → drive → amp → cab → delay」這種三顆疊加 drive
的板子，今天做不到。

白皮書 §4.2 的模型本來就是**可重排線性鏈 `Vec<EffectSlot>`**，§4.3 預留
了 `effect_kind:instance:param` 的 instance 語意——本 PRD 是把 slot
集合開放給使用者，完成白皮書藍圖，不是偏離。

## 2. 目標

1. 鏈上可自由組板：**拖拉重排、新增、移除** family 實例；同 family 可
   多實例。
2. 每個實例點進去獨立選 pedal、調參數（PRD 001 的體系，逐實例獨立）。
3. **鏈結構存入 preset**：v3 chain 陣列的順序與重複即結構，載入即重建。
4. 結構變更聽感安全：騎 master fade 過靜音（白皮書 §4.2 拓撲切換規則），
   **未動的 slot 狀態（delay/reverb 尾音）保留**。

## 3. 規則與限制

- slot 總數上限 **12**（`MAX_SLOTS`，定長訊息所需；常數可再放大）。
- **amp、cab 各至多一顆**——NAM/IR 資產掛載點是單例 seam；解除限制留待
  後續（需要 per-instance asset handle）。
- 空鏈合法（直通）。
- limiter 不再強制最後、可被移除——白皮書 §3.3「常駐輸出 limiter」的
  安全保證**移到輸出級**（PRD 003 的 safety limiter 承接）。

## 4. 非目標

- plugin 內的結構編輯：host 參數表無法動態化，plugin v1 維持固定預設
  鏈；後續 PRD 處理。
- 平行分支 / wet-dry 路由（白皮書 §4.2 既有非目標）。
- per-instance 的 NAM/IR 多實例。

## 5. 使用者故事

- 我把 drive 卡片按住拖到 comp 後面，放開，鏈變了，聲音只淡出淡入一瞬。
- 我按「＋」加第二顆 drive，選 centaur 當 always-on boost，第一顆 ts9
  推 solo。
- 我把整條板子存成 preset，重開 app 載入，三顆 drive 連同各自 pedal
  與旋鈕全部回來。
- 我在 REPL 打 `set drive2.gain 6`，動的是第二顆 drive。

## 6. UX 規格（GUI）

- **鏈條卡片列即編輯器**：
  - 按住卡片拖動 → 目標間隙顯示插入預覽 → 放開提交重排。
  - 點一下選取；下方參數面板 = pedal 下拉 + 旋鈕（PRD 001）＋
    「◀ ▶」微調位置＋「移除」。
  - 列尾常駐「＋」卡片 → family 選單（amp/cab 已存在或達 12 上限的
    項目停用並註明原因）。
- 卡片顯示 family 名 + 現用 pedal 名（如 `Drive · TS9`）+ on/bypassed。
- 空鏈顯示「空鏈（直通）——按＋開始組板」。
- live view 的 chain summary 顯示實例序列。

## 7. 技術設計

### 7.1 lh-engine：slot 熱插拔協定

- `Chain.slots: Vec<Option<Slot>>` 固定容量 `MAX_SLOTS`；`order` 引用
  存活索引。索引 = 實例的穩定 handle（移除後可重用）。
- 新訊息：
  - `InstallSlot { index, effect: Box<dyn Effect> }`——effect 由控制端
    建構並 `prepare` 完成（配置都在控制端），RT 端只做指標安放。
  - `RemoveSlot { index }`。
- 被替換/移除的 effect 經 **retire ring**（garbage chute，同白皮書
  §4.1 資產模式）送回控制端 drop——RT 執行緒永不釋放記憶體。chute 滿
  時 park（同 `AssetSlot` 行為），絕不丟棄、絕不阻塞。
- 結構訊息與 `SetOrder` 一樣騎 master fade：淡出 → 底部套用 → 淡入。
  一次編輯 = install/remove + 新 order 成批送出，於同一次靜音底部生效。
- `ChainHandle` 需知道串流 sample rate 才能 prepare 新 effect
  （Session 於 runner 起流後回填）。

### 7.2 實例定址

- 控制面 handle：`family`（該 family 第一顆）或 `familyN`
  （同 family 依鏈序 1-based：`drive`、`drive2`、`drive3`）。
- GUI 內部用 slot 索引；REPL/MIDI 用 handle 字串。
- handle 隨鏈序變動（拖動第三顆到最前，它變成 `drive`）——語義文件化：
  MIDI 映射是「板位」不是「那一顆」，與實體 looper/switcher 的行為一致。

### 7.3 Preset

- v3 `chain` 陣列 = 結構真相（順序、重複、每實例的 pedal + 參數 +
  bypass）。
- **載入 = 結構調和**：與現行鏈逐位置比對 family——相同者保留實例
  （參數照舊套用，尾音不斷）；不同者 replace；多的 remove；缺的
  install。全程一次 fade。
- v2 檔（固定十格）遷移為等價結構；未知 family 略過 + 警告。
- 儲存沿用 snapshot：實例順序 + 全 pedal 參數（PRD 001 §7.3）。

### 7.4 REPL

```
add <family> [pos]      在 pos（預設尾端）插入
remove <handle>         移除實例
order <handle> ...      全鏈重排（handle 列表）
pedal <handle> <name>   選 pedal
set <handle>.<param> v  / on <handle> / off <handle>
```

### 7.5 MIDI

- cc 目標支援 handle（`"11": "drive2.gain"`、`"80": "drive2"`）。
- 解析失敗（該位置不存在）忽略並回報一行。

## 8. 相容性與風險

| 風險 | 對策 |
| --- | --- |
| 結構訊息與參數訊息交錯造成錯位 | 單一 SPSC ring 保序；編輯成批送出、fade 底部生效 |
| retire chute 滿導致洩漏 | park + 下一 block 重試（既有 AssetSlot 模式），控制端每 frame collect |
| 拖拉誤觸 | 需按住並位移 > 閾值才進入拖曳；點擊仍是選取 |
| v2 preset 行為差異 | 遷移測試鎖定「等價結構、等價聲音」 |
| 12 上限太緊 | 常數集中一處，放大只動一行 + ring 容量檢查 |

## 9. 驗收標準

1. 組出 comp→drive→drive→drive→amp→cab→delay；三顆 drive 各選不同
   pedal、參數互不干擾；preset 存檔重載完全還原（結構 + pedal + 值）。
2. 演奏中新增/移除/拖拉 slot：無爆音（master fade 生效）、未動 slot 的
   delay 尾音持續；debug 建置下 assert_no_alloc 不觸發。
3. 空鏈直通可運作；12 上限與 amp/cab 單例在 GUI 與 REPL 都被擋下且有
   訊息。
4. v2 preset 載入 → 等價舊鏈結構、聲音不變。
5. REPL `set drive2.gain 6` / MIDI `"drive2.gain"` 正確定址第二顆。
6. 全 gates 綠（fmt/clippy/test）；引擎新增協定有單元測試
   （install/remove/reorder/尾音保留/chute 回收）。
