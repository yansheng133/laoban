# laoban-m0

試吃版（M0）的後端流程：一句需求 → AI 計畫與架構圖 → 確認 → AI 產生程式 → 套固定範本 → commit 到 GitHub。規格以 `docs/m0-design.md` 為準；本 crate 不寫入任何叢集，部署由 Fleet 負責。

HTTP 路由在 `laoban-server`；本 crate 只提供與傳輸無關的 `M0Service`（`plan`、`apply`、`config_json`）。

## 元件

| 模組 | 內容 |
|---|---|
| `plan` | 計畫呼叫的回應格式說明、解析與檢查、轉成 3D 檢視器的 `architecture {nodes, edges}` |
| `codegen` | 產生程式呼叫的回應格式說明、四個檔案的白名單與大小上限 |
| `render` | 固定範本：`fleet.yaml`、兩個 ConfigMap、兩組 Deployment＋Service、`README.md`；`templates/server.py`、`templates/main.py` |
| `github` | `GithubCommitter`：以 Git Data API 一次 commit 整個目錄 |
| `store` | 記憶體中的計畫：保留 1 小時、最多 20 筆，超過時淘汰最舊的 |
| `service` | `M0Service` 與錯誤代碼對應 |
| `config` | 環境變數 |

## 兩次 AI 呼叫

兩次都經過 `laoban-ai` 的 `Orchestrator::complete()`：遮蔽（`Destination::Ai`）、資料區塊邊界、全域 token 預算、錯誤分類與不重試規則都和其他呼叫相同。模型 ID 來自 `LAOBAN_MODEL`。

1. **計畫**（`max_tokens` 4096）：使用者需求先遮蔽，放在 `kind=user_requirement` 資料區塊；回應格式寫在系統提示詞。回應必須是 `{appName, summary, steps[], components:{web:{modules[]}, api:{modules[], endpoints[]}}, notes}`：
   - 可以包在一層 Markdown code fence 裡（會剝掉）；多出來的欄位（例如 AI 自己給的座標）忽略。
   - `modules` 每項可以是字串或 `{name, purpose}`；每個元件最多 8 個模組、最多 16 個端點、最多 20 個步驟；`method` 只能是 GET／POST／PUT／PATCH／DELETE，`path` 必須以 `/` 開頭、只含安全字元。
   - `notes` 可以是字串或字串陣列（會合併）。
   - `appName`：請求有給就用請求的（不合規則回 400）；沒給就用 AI 的，不合 `^[a-z][a-z0-9-]{1,30}$` 回 `ai_invalid_output`。
2. **產生程式**（`max_tokens` 16000）：把計畫（JSON）放在 `kind=approved_plan` 資料區塊。回應只能是 `{"web": {"index.html", "app.js", "style.css"}, "api": {"handler.py"}}`，鍵必須剛好這四個，多一個、少一個、值不是字串都回 `ai_invalid_output`；每個檔案上限 64 KiB、合計 192 KiB；`handler.py` 必須有 `def handle(`。被截斷（`max_tokens`）或拒答的回應同樣回 `ai_invalid_output`，不會寫入 GitHub。

## 架構圖

AI 只決定名稱、模組、端點與說明；圖的形狀與座標都由後端決定，同樣輸入一定得到同樣的圖。

| id | layer | kind | 位置 (x, z) |
|---|---|---|---|
| `svc-web` / `svc-api` | network | service（detail 2，parent 指向對應 app） | (-3, -2) / (3, -2) |
| `app-web` / `app-api` | code | app（detail 1） | (-3, 0) / (3, 0) |
| `mod-web-N` / `mod-api-N` | code | module（detail 3） | 在所屬 app 下方，每列 4 個、間距 1.5，從 z = 2.5 起排 |
| `tbl-data` | data | table（detail 2，`props.storage: "memory"`） | (3, 0) |

連線：`svc-web → app-web`（routes）、`app-web → svc-api`（calls）、`svc-api → app-api`（routes）、`app-api → tbl-data`（writes）。

## 產生的檔案（`apps/<appName>/`）

| 檔案 | 內容 |
|---|---|
| `fleet.yaml` | `defaultNamespace` 與 `namespace` 都是 `LAOBAN_TARGET_NAMESPACE` |
| `configmap-web.yaml` | `<app>-web-files`：AI 的三個網頁檔＋範本 `server.py` |
| `configmap-api.yaml` | `<app>-api-code`：AI 的 `handler.py`＋範本 `main.py` |
| `web.yaml` / `api.yaml` | Deployment＋Service `<app>-web` / `<app>-api`，port 8080（Service 用 ClusterIP） |
| `README.md` | 遮蔽後的原始需求、遮蔽後的計畫摘要、產生時間（UTC）、模型 ID |

- 兩個 Deployment：`registry.suse.com/bci/python:3.13`，`command: ["python3", "/app/server.py"]` 或 `/app/main.py`；ConfigMap 唯讀掛在 `/app`，另掛一個 16Mi 的 `emptyDir` 在 `/tmp`。
- **映像用 tag，沒有用 digest**：開發環境連不到 registry.suse.com，無法查證 tag 與 digest，部署前要人工確認並改成 digest。
- Pod：`runAsNonRoot: true`、`runAsUser: 65532`、`seccompProfile: RuntimeDefault`、`automountServiceAccountToken: false`；容器另加 `allowPrivilegeEscalation: false`、`readOnlyRootFilesystem: true`、`capabilities.drop: [ALL]`；requests 50m／64Mi，limits 200m／128Mi。探測用 TCP（不佔用 handler 的路徑）。
- 所有物件都有 `app.kubernetes.io/managed-by: laoban`、`laoban.io/app: <appName>`，另加 `app.kubernetes.io/name`、`app.kubernetes.io/component` 作為 selector。
- pod template 的 `laoban.io/content-hash` 是對應 ConfigMap `data` 的 sha256：依鍵名排序，每筆送入 `鍵 0x00 值 0x00`，十六進位小寫。內容一變就會重新部署。
- ConfigMap 內容能原樣往返時寫成 YAML literal block（`|`），否則（例如 CRLF、沒有結尾換行、開頭空白、控制字元、U+2028）改用雙引號字串並跳脫；label 值一律加引號（避免 `no`、`yes` 被 YAML 1.1 讀成布林值）。測試會用 PyYAML 讀回比對。

### `server.py`（網頁）

只用標準函式庫。只提供 `index.html`、`app.js`、`style.css` 三個檔（`server.py` 自己不公開，其他路徑一律 404），Content-Type 正確；`/` 回 `index.html`。`/api` 與 `/api/*` 轉給 `LAOBAN_API_URL`（範本設為 `http://<app>-api:8080/`），去掉 `api/` 前綴，保留方法、主體、查詢字串、`Content-Type`，原樣回傳狀態碼與主體；API 連不上回 502。不使用環境中的 proxy 設定。

Rancher 服務代理會把 `/k8s/clusters/.../proxy` 前綴去掉再轉給 Pod，所以伺服器只看根路徑；頁面本身必須全部用相對網址（`style.css`、`app.js`、`api/...`），產生程式的提示詞有明確要求。

### `main.py`（API）

`http.server` 包住 `handler.handle(method, path, body) -> (status, dict)`：`path` 不含 `api/` 前綴與查詢字串；JSON 進出；例外或不合法的回傳值一律回 500 `{"error": "internal error"}`，traceback 只寫到容器 log。一次處理一個請求，所以 handler 的模組層級資料不需要鎖。

## GitHub

`GithubCommitter::commit()` 依序呼叫：取得 `refs/heads/main` → 取得該 commit 的 tree → 每個檔案建立 blob（`encoding: utf-8`）→ 以 `base_tree` 建立 tree → 建立 commit（parent 是原本的 head）→ 以 `force: false` 更新 ref。commit message：`laoban: <appName> — <遮蔽後摘要前 60 字>`，空一行後加 `Generated-by: laoban`。

- 不重試：任何一步失敗（包含分支在這段時間被別人推進，GitHub 回 422 或 409）都回 `github_failed`，由使用者自己再按一次。
- token 來自 `LAOBAN_GITHUB_TOKEN`，`Debug` 顯示 `[REDACTED]`，不會出現在錯誤或 log；錯誤只帶 GitHub 回應的 `message` 欄位（截到 200 字）。
- 目標 repo 必須已經有 `main` 分支（至少一個 commit）。
- API 位址可設定（`GithubConfig::api_base`），測試用本機模擬伺服器。

## 計畫的狀態

- 同一個 `planId` 成功 apply 後不能再 apply（`409 already_applied`）；apply 進行中再送一次也是 409。
- apply **失敗**（AI 回應不合格式、GitHub 失敗等）時計畫回到可 apply 的狀態，使用者可以直接再按一次，不必重新產生計畫。
- 存放的只有遮蔽後的需求；原始需求不保存。

## 錯誤代碼

| 代碼 | HTTP | 情況 |
|---|---|---|
| `bad_request` | 400 | 需求空白或超過 4000 字、`appName` 不合規則、JSON 格式不對 |
| `plan_not_found` | 404 | 沒有這個計畫、已過 1 小時、被淘汰或後端重啟過 |
| `already_applied` | 409 | 已部署過或正在部署 |
| `budget_exceeded` | 429 | laoban 自己的 token 上限（呼叫前就拒絕，不會呼叫 Anthropic） |
| `spend_limit` | 402 | Anthropic 花費上限（組織／workspace 自訂上限或用量層級上限），不重試 |
| `ai_invalid_output` | 502 | AI 回應不合格式、超過大小、多了不允許的檔案、被截斷或拒答 |
| `github_failed` | 502 | Git Data API 任一步失敗 |
| `internal` | 500 | 設定不完整、AI 服務忙碌（重試後仍失敗）、認證失敗或其他呼叫失敗 |

## 設定

| 變數 | 必要 | 預設 |
|---|---|---|
| `ANTHROPIC_API_KEY` | 是 | |
| `LAOBAN_MODEL` | 是 | |
| `LAOBAN_GITHUB_REPO` | 是（`<owner>/<repo>`） | |
| `LAOBAN_GITHUB_TOKEN` | 是 | |
| `LAOBAN_TOKEN_CAP` | 否 | 2000000（每 UTC 曆月） |
| `LAOBAN_TARGET_NAMESPACE` | 否 | `laoban-dev` |
| `LAOBAN_CLUSTER_ID` | 否 | `c-m-26kbxk5f` |

缺少或格式不對時，錯誤只寫變數名稱，不寫值。Fleet 的 workspace（`fleet-default`）與 GitRepo 名稱（`laoban-apps`）依規格固定在程式裡。

## 測試

```bash
cargo test -p laoban-m0
python3 -m unittest discover -s crates/laoban-m0/python-tests   # server.py／main.py 端對端
```

- `tests/pipeline.rs`：模擬 provider 與模擬 GitHub，涵蓋 plan → apply、重複 apply、找不到計畫、多餘檔案、檔案過大、花費上限只呼叫一次、預算不足時完全不呼叫、身分證字號不會送到 AI 也不會出現在 commit 的 README、分支過期（422／409）不重試。
- `tests/python.rs`：有 python3 時執行上面的 Python 測試；有 PyYAML 時把產生的 manifest 讀回比對。沒有就略過並印出 `SKIPPED`（CI 的 Rust 映像不一定有 python3）。

測試資料全部是合成的：repo 用 `example-owner/laoban-apps`、token 與金鑰是假字串、身分證字號以檢查碼產生。
