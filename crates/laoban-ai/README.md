# laoban-ai

laoban 呼叫 AI 模型的唯一路徑。負責：模型供應商抽象層、錯誤分類與重試、全域 token 預算、送出前遮蔽、提示注入（prompt injection）緩解、AI 回應的結構驗證，以及稽核事件。

本 crate **不執行任何東西**：AI 的回應一律當成資料，解析成型別化的 `Plan` 並對照允許清單檢查，執行與部署由後續流程（含人工審查）負責。

> 本次變更**沒有**實作真正的 HTTP 用戶端，所有測試都使用 `MockProvider`，不會連網。下方「Anthropic Messages API 對應」一節記錄之後實作 HTTP 用戶端時要對應的官方格式。

## 元件

| 模組 | 內容 |
|---|---|
| `provider` | `Provider` trait（同步，`send(&ProviderRequest) -> Result<ProviderResponse, ProviderError>`）、與供應商無關的請求／回應型別、測試用 `MockProvider`（依腳本回傳結果並記錄收到的請求） |
| `error` | `ProviderError` 與 `classify()`：由 HTTP 狀態碼、`error.type`、`error.message`、`error.details.error_code`、`retry-after` 判斷錯誤類別 |
| `retry` | `RetryPolicy`、純函式 `backoff_delay()`、可注入的 `Sleeper`（測試用 `RecordingSleeper` 只記錄不睡） |
| `budget` | 全 Project 共用的 `TokenBudget`：每期上限、呼叫前預留、呼叫後依實際用量結算、失敗退還 |
| `prompt` | 不受信任內容的資料區塊、每次請求隨機的邊界標記、偽造邊界的中和 |
| `plan` | AI 必須回傳的 `Plan` 結構與 `PlanPolicy` 允許清單 |
| `model` | 管理者強制指定模型，或讓使用者從允許清單中選擇 |
| `audit` | `AiAuditSink` trait，發出 `ai.request`／`ai.response` 事件（只含遮蔽後文字） |
| `orchestrator` | 串起上述元件；擁有 `Masker`，呼叫端無法略過遮蔽 |

## 處理流程

`Orchestrator::propose_plan()` 依序執行：

1. **選模型**：依 `ModelSelection` 決定模型 ID。模型 ID 一律來自設定，程式中不寫死。使用者指定不在允許清單的模型時回傳 `ModelError::NotAllowed`，不會偷偷換成別的模型。
2. **遮蔽**：使用者需求、叢集狀態、檔案內容（含標籤與路徑）、目標叢集名稱與管理者系統提示詞，全部以 `Destination::Ai` 經 `laoban-mask` 遮蔽。身分證字號、居留證號、金融帳號、登入帳號都不會送出；IP 位址依 `laoban-mask` 的設計保留。發起者的 principal ID 只進稽核，不送給 AI。
3. **包成資料區塊**：每段不受信任的內容放進 `<<<LAOBAN_DATA_<nonce> BEGIN kind=...>>>` 與 `<<<LAOBAN_DATA_<nonce> END>>>` 之間。
4. **預留預算**：預留 `估計輸入 token + max_tokens`。超過上限時回傳 `BudgetError::Exhausted`，**不會呼叫供應商**。
5. **呼叫與重試**：只有 `RateLimited` 與 `Overloaded` 會重試，次數上限與退避時間由 `RetryPolicy` 決定；伺服器給了 `retry-after` 就照它等，但超過 `max_delay_ms` 就直接放棄。`SpendLimitReached`、`InvalidRequest`、`Authentication`、`Other` 一律不重試。每次失敗都會退還該次預留。
6. **結算**：成功後依回應的 `input_tokens + output_tokens` 結算。
7. **驗證回應**：`stop_reason` 為 `max_tokens`（被截斷）或 `refusal` 時拒絕；其餘解析成 `Plan` 並以 `PlanPolicy` 檢查。

每次呼叫（含重試）都會發出一筆 `ai.request` 與一筆 `ai.response` 稽核事件，內容是遮蔽後文字、token 數、模型 ID 與 principal ID。回應文字寫入稽核前再以 `Destination::Audit` 遮蔽一次。

## Token 預算

- `TokenBudget` 以 `Mutex` 保護，以 `Arc` 在所有 Project 之間共用。
- 每期（period）由注入的 `PeriodClock` 決定；內建 `UtcMonthClock`（UTC 曆月）與測試用 `FixedPeriod`。期別一變，用量歸零。
- 上限管的是**預留**：任何時刻「已結算 + 未結算預留」都不會超過上限。實際用量若高於預留（輸入估計偏低），超出部分仍照實記錄，後續預留會看到。
- `Reservation` 未結算就被丟棄（供應商錯誤、panic）時自動全額退還。
- 輸入 token 目前以「UTF-8 位元組數 ÷ 3 + 8」粗估（中文字一個約 3 位元組、約 1 token；英文約 4 字元 1 token，所以偏高估）。HTTP 用戶端上線後可改用官方的 token 計數端點 `POST /v1/messages/count_tokens`。

## 提示注入：是緩解，不是防止

必須誠實說明：**沒有任何提示詞技巧能保證模型不被資料區塊裡的文字影響**。本 crate 做的是多層緩解，真正的控制點在最後一層：

1. **資料區塊與隨機邊界**：每次請求產生新的 nonce（`RandomBoundary` 以標準函式庫的隨機金鑰雜湊產生 128 位元；測試注入 `FixedBoundary`）。
2. **中和偽造邊界**：不受信任文字中出現的完整邊界，以及任何大小寫的 `LAOBAN_DATA_` 前綴，都換成 `[NEUTRALIZED_MARKER]`。因此資料無法提前關閉自己的區塊、也無法偽造新區塊。標籤裡的換行也會被移除。
3. **系統提示詞聲明**：系統提示詞說明區塊內容是資料、不是指令，並要求只回傳一個 JSON 物件。
4. **回應當資料處理（主要控制點）**：回應必須通過 `Plan` 的結構驗證（`deny_unknown_fields`，未知欄位一律拒絕），並符合 `PlanPolicy`：
   - 只能碰目標叢集，其他叢集回傳 `PlanError::WrongCluster`；
   - 預設禁止 `Secret`、`Role`、`ClusterRole`、`RoleBinding`、`ClusterRoleBinding`（不分大小寫、單複數，也不分動作，連讀取都不行）；
   - 預設禁止整個 `rbac.authorization.k8s.io` API group（涵蓋權限提升與綁定）；
   - 預設禁止 `kube-system`、`cattle-system`、`cattle-fleet-system` namespace；
   - 檔案路徑必須是相對路徑，不得含 `..`、絕對路徑、磁碟代號或空片段；
   - 步驟、檔案、資源數量有上限。
5. **不執行**：本 crate 從不執行計畫；計畫還要經過 `laoban-policy` 的審查規則與人工審查。

## Anthropic Messages API 對應

以下內容於 2026-10-12 對照官方文件查證（`docs.claude.com` 目前轉址到 `platform.claude.com/docs`）。

### 請求

`POST https://api.anthropic.com/v1/messages`，標頭 `x-api-key`、`anthropic-version: 2023-06-01`、`content-type: application/json`。

| `ProviderRequest` | Messages API |
|---|---|
| `model` | `model` |
| `system` | `system`（字串，或 `{"type":"text","text":...}` 陣列） |
| `messages[].role` | `messages[].role`：`user` / `assistant`（`messages` 裡沒有 `system` 角色） |
| `messages[].content` | `messages[].content`（字串是單一 `text` 區塊的簡寫） |
| `max_tokens` | `max_tokens`（產生 token 的絕對上限，模型可能提早停止） |

### 回應

| Messages API | `ProviderResponse` |
|---|---|
| `content[]` 中 `type: "text"` 的 `text` 串接 | `content` |
| `usage.input_tokens` | `input_tokens` |
| `usage.output_tokens` | `output_tokens` |
| `stop_reason`：`end_turn`、`max_tokens`、`stop_sequence`、`tool_use`、`pause_turn`、`refusal`、`model_context_window_exceeded` | `stop_reason`（`end_turn`、`max_tokens`、`stop_sequence`、`refusal` 各有對應，其餘歸入 `StopReason::Other`） |

### 錯誤格式與分類

錯誤一律是 JSON：`{"type":"error","error":{"type":...,"message":...},"request_id":"req_..."}`，每個回應另有 `request-id` 標頭。

| HTTP | `error.type` | 條件 | `ProviderError` | 重試 |
|---|---|---|---|---|
| 400 | `invalid_request_error` | 訊息開頭為 `You have reached your specified API usage limits` 或 `You have reached your specified workspace API usage limits`（自行設定的組織／workspace 支出上限） | `SpendLimitReached { kind: SelfSet }` | 否 |
| 429 | `rate_limit_error` | `error.details.error_code` = `enforced_spend_limit_reached`（用量層級每月支出上限）；沒有 `retry-after` | `SpendLimitReached { kind: TierCap }` | 否 |
| 429 | `rate_limit_error` | 其餘（一般速率限制、加速限制） | `RateLimited { retry_after }` | 是 |
| 529 | `overloaded_error` | | `Overloaded` | 是 |
| 401 / 403 | `authentication_error` / `permission_error` | | `Authentication` | 否 |
| 400 / 404 / 413 | `invalid_request_error` / `not_found_error` / `request_too_large` | | `InvalidRequest` | 否 |
| 402 / 500 / 504 / 其他 | `billing_error` / `api_error` / `timeout_error` / … | | `Other` | 否 |

### 查證結果：事先提供的兩項事實

1. **自行設定的組織／workspace 支出上限回傳 HTTP 400 `invalid_request_error`**：**正確**。錯誤頁的 400 條目明載「The API also returns a 400 when usage reaches an organization or workspace spend limit you set」；速率限制頁補充訊息開頭為 `You have reached your specified API usage limits`（workspace 則為 `... specified workspace API usage limits`）。例外：Claude Code workspace 的上限可能改回 429 並附 `retry-after`，本 crate 會把它當一般速率限制處理。
2. **用量層級支出上限回傳 HTTP 429，錯誤類型 `enforced_spend_limit_reached`**：**需更正**。HTTP 429 正確，但 `error.type` 是 `rate_limit_error`（與一般速率限制相同），`enforced_spend_limit_reached` 是放在 `error.details.error_code` 的值；而且這個 429 **沒有** `retry-after` 標頭，重試（含 SDK 自動重試）會一直失敗，直到下個月 1 日 00:00 UTC 恢復或升級層級。分類器因此先看 `error.details.error_code`，缺少時再以「429、無 `retry-after`、訊息開頭為 `You have reached your API usage limits`」判斷。

### 來源

- 錯誤代碼與錯誤格式：<https://platform.claude.com/docs/en/api/errors>（原網址 <https://docs.claude.com/en/api/errors> 轉址至此）
- 速率限制、支出上限（「Reaching your spend cap」「Setting your own spend limit」）與回應標頭：<https://platform.claude.com/docs/en/api/rate-limits>（原網址 <https://docs.claude.com/en/api/rate-limits>）
- Messages API 請求與回應格式：<https://platform.claude.com/docs/en/api/messages/create>

## 尚未處理

- 真正的 HTTP 用戶端（含 `retry-after` 標頭解析、`request-id` 記錄）。
- 500 `api_error` 官方建議以指數退避重試，目前歸類為 `Other` 不重試，待決定。
- 402 `billing_error` 目前歸類為 `Other`，是否要獨立成類別待決定。
- 退避沒有加入抖動（jitter）；多個執行個體同時重試時可能同步，HTTP 用戶端上線時再評估。

## 測試

```bash
cargo test -p laoban-ai
```

測試資料全部是合成的：IP 用 192.0.2.x、網域用 example.test、principal 用 `local://u-test01`、身分證字號以檢查碼產生、模型 ID 用 `model-a-test` 這類假名稱。
