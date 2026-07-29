# ADR 037: Tone stack makeup 重校 — noon 天花板取代頻帶平均

狀態：**已採納（2026-07-29）**
關聯：ADR 030（tone stack 引擎——本 ADR 取代其 makeup 校準點）、PRD 023（落地規格，
表格中的 makeup 值由本 ADR 更新）、`docs/tone_revolution/phase/02-tonestack-framework.md`
§4.4（耳朵驗收——本 ADR 是該項驗收不通過的處置）
影響範圍：`eq::tonestack::KINDS` 三個 `makeup_db`、五顆 FMV-voiced drive
（`evva`/`red-charlie`/`monster5150`/`angry-charlie`/`angry-charlie-v2`）、
`mane`（內建 jcm800 stack）、`big-muff`（內建 big-muff stack）、`eq` 家族
`tonestack` 踏板

## Context

Tone Revolution 交付後的耳朵驗收（phase 02 §4.4）不通過，症狀具體：**低頻溢出、
錄音介面錶頭爆掉，且調小 drive 與 level 都救不回來**。量測復現如下。

用 82.4 Hz 低音弦擬真訊號（撥弦與悶音連擊、−6 dBFS 峰值）對整個 drive 家族做
頻帶分析，並把 phase 02 落地前一個 commit（`bd2de3b~1`）開成 worktree 跑同一組
量測做 A/B。同旋鈕、同訊號下，**40–160 Hz 頻帶的絕對電平**：

| 踏板 | stack | 遷移前 | 遷移後 | Δ 低頻帶 | Δ 中頻帶 (160 Hz–1k) |
|---|---|---|---|---|---|
| `evva` | bassman | −18.2 dB | −13.1 dB | **+5.1 dB** | +0.7 dB |
| `red-charlie` | jcm800 | −21.1 | −17.1 | **+4.0** | +1.4 |
| `monster5150` | jcm800 | −24.5 | −20.5 | **+4.0** | +1.6 |
| `angry-charlie` | jcm800 | −22.2 | −18.2 | **+4.0** | ≈0 |
| `angry-charlie-v2` | jcm800 | −25.2 | −21.2 | **+4.0** | ≈0 |
| `ts9`/`bd2`（對照組） | — | — | — | 0.0 | 0.0 |

峰值同步 +3～4 dB。**低頻是被絕對性地抬高，不是整體變大聲**——這解釋了兩個
「沒用」：抬升發生在 clipper 之後的線性 stack，所以 drive 旋鈕管不到；level 是
等比縮放、不改頻譜傾斜，把整體壓回可用音量時低頻仍然先撐爆錶頭。

機制在 makeup 的校準點。ADR 030 的 makeup 定義為「80 Hz–7.2 kHz **頻帶平均**增益
取負」，由 `noon_sits_near_unity_with_the_makeup_applied` pin 住。但 noon 曲線
中間凹 7–9 dB：把**平均**校到 unity，代數上就是把兩端 shelf 推到 unity **以上**。
實測 noon 絕對增益（含當時 makeup）：bassman **+5.1 dB @ 82 Hz**、jcm800
**+4.0**、big-muff +1.9——與上表五顆踏板的低頻增量分毫不差。

把 makeup 拿掉，三個網路的曲線就落回公開的實測響應（5F6-A noon：低頻 shelf 約
−2 dB、凹陷約 −12 dB）——**netlist 與狀態空間引擎是對的，錯的只有 makeup 這一個
校準常數**。真實被動網路處處 ≤ unity，「低頻 shelf 高於輸入」是任何被動 stack
都做不出來的響應。

同一輪量測也**排除了其他嫌疑**：phase 04/05/08 的 WDF／transistor 踏板在隔離
量測中低頻帶絕對電平全部低於輸入、8 Hz 次聲波增益全為負、level 旋鈕線性有效——
使用者聽感上「新 drive 普遍溢出」的主體就是這五顆同期改聲的 FMV 踏板（以及內建
同款 stack 的 `mane`）。

Phase 02 的驗收條款漏了這個面向：§4.1 只 pin 了 RMS 級的 level-norm（量測點
220 Hz 恰好在曲線支點附近，+0.9 dB，全綠），沒有任何一條檢查**每頻帶絕對電平**
對照遷移前。護欄本身缺席，所以錯的校準點一路綠燈到耳朵驗收才被接住。

## Decision

**makeup 改為「noon 天花板 = unity」**：取 netlist 在 noon、15 Hz–16 kHz 上的
**最大**增益（即低頻 shelf）取負。noon 響應因此處處 ≤ 0 dB——和被動網路本身一樣
——留在天花板以下的就是凹陷本體。

以獨立 AC oracle（`nodal_db`，解析、與取樣率無關）在 200 點對數網格上求得：

| Kind | 原始 noon 最大值 | 舊 makeup | **新 makeup** |
|---|---|---|---|
| `bassman` | −1.63 dB @ 42.7 Hz | +7.38 | **+1.63** |
| `jcm800` | −0.98 dB @ 47.4 Hz | +5.32 | **+0.98** |
| `big-muff` | −4.05 dB（DC 漸近線） | +6.13 | **+4.05** |

校準測試 `noon_sits_near_unity_with_the_makeup_applied`（頻帶平均 |avg| < 0.5）
**替換**為 `noon_ceiling_sits_at_unity_with_the_makeup_applied`：同一組細網格求
原始最大值，斷言 `max + makeup` 在 ±0.25 dB 內——同時是校準 pin 與「任何頻點不
得超過 unity」的護欄，失敗訊息直接給出重校值。

### 驗證

重跑同一組 A/B 量測，五顆踏板的 40–160 Hz 頻帶回到遷移前 **0.7 dB 以內**
（evva −18.9 vs −18.2、red-charlie −21.4 vs −21.1、monster5150 −24.9 vs −24.5、
angry-charlie −22.5 vs −22.2、v2 −25.6 vs −25.2）；noon 每頻點增益最大值
−0.18／−0.05／−0.09 dB，處處 ≤ unity。凹陷深度、旋鈕互動、耦合等**相對**性質
全部不變（那些測試原樣通過）。

### 連帶重 pin（兩處，其餘 466 個測試原樣通過）

- `modelled_pedals_sit_near_unity_at_default_knobs`：±6 dB unity 視窗**原樣保留
  且全數通過**（重校後五顆落在 −1.2～−4.0 dB）。原本跨全家族的 spread < 5 dB
  斷言改為**按 voicing 群組**各自 < 5 dB：scoop 把中段能量移到肩上，220 Hz 單音
  探針天生少讀它幾 dB，跨群一刀切的 spread 量的是探針的盲區而不是切換舒適度。
- `evva_noon_carries_the_bassman_scoop`：sane-level 視窗 (0.2..5.0) → (0.1..2.0)。
  天花板校準下凹陷底騎完整 Bassman 深度（整顆踏板 ~−16 dB @ 750 Hz），不再被
  平均校準撐高；上限收到 2.0 是因為 noon 處處 ≤ unity 後，2× 以上就是 bug。

## Consequences

**好的**

- 低頻回到遷移前水位，錶頭不再被 stack 自己推爆；drive/level 旋鈕的行為恢復
  直覺。
- noon 曲線與真實被動網路同構（處處 ≤ unity），「netlist 是事實」的白皮書立場
  重新成立——makeup 不再發明被動網路做不出的響應。
- 護欄測試從「平均近 unity」升級為「天花板在 unity」，這一類校準錯誤從此在
  `cargo test` 就被接住，不用等耳朵。

**要付的**

- 五顆 FMV 踏板（及 `mane`、`big-muff`）在 noon 的整體響度降 ~2–6 dB（bassman
  −5.75、jcm800 −4.34、big-muff −2.08，落在各踏板上依其頻譜而定）。這是拿掉
  虛增低頻後的誠實響度；`LEVEL_MAX_LIN` = +9 dB，level 旋鈕餘裕足以補回。
- 家族切換的響度一致性現在是**每群組**的主張，不是全家族一刀切。
- 既有 preset 若配平過這五顆的 level，會感覺變安靜、低頻變緊——這正是本 ADR 的
  目的；重新配平即可（或跑 `level` 重測 per-preset trim）。

**沒做的**

- 不動 netlist、不動引擎、不動凹陷深度與旋鈕互動——只動一個校準常數與其 pin。
- 量測中順帶記錄、與本 ADR 無關的兩個觀察，留待各自處理：`rangemaster` 在
  drive 8 悶音下峰值 −0.19 dBFS（booster 對 0 dBFS 的 headroom）；`fuzz-face`
  悶音下 40 Hz 以下能量佔 10.5%（包絡性 bias 抽動，行為模型固有）。
- 每頻帶絕對電平的家族級回歸測試（本次診斷 harness 的正式化）未隨本 ADR 落地
  ——天花板護欄已擋住這一類成因；若日後再出現「頻譜傾斜跑掉」類回歸再補。
