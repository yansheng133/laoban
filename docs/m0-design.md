# 試吃版（M0）設計

試吃版只做一條最短的路，讓人能實際用用看：**一句需求 → AI 計畫與 3D 架構圖 → 確認 → 程式寫進 GitHub → Fleet 部署 → 打開連結**。這份文件是後端、extension 與部署三邊共用的規格；三邊有衝突時以這裡為準。

## 範圍

| 做 | 不做（延到第一階段） |
|---|---|
| 單一使用者（Sam），用自己的 Rancher 登入 | 身分模擬、多角色、審查流程 |
| 單一目標：rke2 叢集的 namespace `laoban-dev` | 多環境層、正式層、預覽環境 |
| 產生的應用固定兩個元件：網頁＋API | 資料庫、佇列、任意架構 |
| 程式放 ConfigMap，跑在固定的 SUSE BCI Python 映像上 | 映像建置、cosign、SUSE Security 准入 |
| 送 AI 前遮蔽、全域 token 上限、花費上限不重試 | 稽核紀錄外送（只寫後端自己的 log） |
| 只有繁體中文介面 | 英文、日文 |
| 版本就是 Git commit；退版用 `git revert` | 版本 CRD、3D 圖拖曳存版 |

## 流程

```
瀏覽器（Rancher UI 裡的 laoban 頁面）
  │ 1. POST /api/plan  {prompt}
  ▼
laoban 後端（rke2, ns laoban-system）── 遮蔽 ──► Anthropic Messages API
  │ 回 plan（摘要、步驟、架構圖）
  ▼
使用者看計畫與 3D 圖，按「確認」
  │ 2. POST /api/apply {planId}
  ▼
後端第二次呼叫 AI 產生程式 → 套上固定範本產生 manifest → commit 到 GitHub
  │ 回 commit 連結與應用網址
  ▼
Fleet（Rancher local 叢集）拉 GitHub → 部署到 rke2 / laoban-dev
  │ 3. 瀏覽器用使用者自己的登入讀 Fleet Bundle 狀態，Ready 後顯示連結
```

瀏覽器呼叫後端走 Rancher 的服務代理，用的是使用者自己的登入；後端**不寫入任何叢集**，部署全由 Fleet 做。

## 後端 API

基底路徑：`/k8s/clusters/c-m-26kbxk5f/api/v1/namespaces/laoban-system/services/http:laoban:8080/proxy`（以下簡寫為 `/`）。全部 JSON，UTF-8。

### `POST /api/plan`

請求：

```json
{ "prompt": "做一個早餐店點餐頁面，可以選餐點和送出訂單", "appName": "breakfast" }
```

- `appName`：`^[a-z][a-z0-9-]{1,30}$`。省略時由後端從 AI 回應的 `appName` 取，再驗證。名稱已經被現有應用使用時：使用者指定的回 `409 app_exists`（不呼叫 AI）；AI 取的名稱自動改用 `-2`～`-9` 結尾的空名稱。
- `baseApp`：修改現有應用時帶入它的名稱（不可與不同的 `appName` 同時出現）。後端從 GitHub 讀該應用的 `laoban.json`，把目前的計畫與這次的修改要求分成兩個資料區塊（`current_plan`、`change_request`）給 AI，要求回傳完整的新計畫，名稱不變。回應多一個 `"mode": "edit"`（新應用為 `"create"`）。程式超過 40 KiB 的應用不能編輯（本機模型的 context 放不下）。

回應 `200`：

```json
{
  "planId": "p-20261013-1a2b3c",
  "appName": "breakfast",
  "summary": "…",
  "steps": ["…"],
  "architecture": { "nodes": [ … ], "edges": [ … ] },
  "maskedFindings": 0,
  "usage": { "inputTokens": 1200, "outputTokens": 800 }
}
```

- 端點路徑是 API 程式（`handler.handle()`）看到的路徑；AI 寫成 `/api/orders` 這種瀏覽器端路徑時，後端會去掉開頭的 `/api`（實測本機模型偶爾會這樣寫，與它產生的程式不一致）。
- `architecture` 用 `web/viewer-prototype/graph-diff.mjs` 的模型：`node = {id, layer: network|code|data, kind, name, detail: 1..3, parent?, props?, pos:{x,z}}`，`edge = {from, to, kind, label?}`。
- 試吃版的架構固定是：network 層一個 `service` web、一個 `service` api；code 層一個 `app` web、一個 `app` api，各自可以有 `module`（detail 3）；data 層在 M0 可以有一個 `table` 代表 API 的記憶體資料（`props.storage: "memory"`）。AI 只決定名稱、模組、說明和連線標籤；**位置由後端依層與順序排**，不信任 AI 給的座標。
- 計畫存在後端記憶體（以 `planId` 為鍵，保留 1 小時，最多 20 筆）。後端重啟就消失，可以接受。

### `POST /api/apply`

請求：`{ "planId": "p-…" }`

回應 `200`：

```json
{
  "commit": "abc1234…",
  "commitUrl": "https://github.com/<owner>/<repo>/commit/abc1234…",
  "path": "apps/breakfast",
  "appUrl": "/k8s/clusters/c-m-26kbxk5f/api/v1/namespaces/laoban-dev/services/http:breakfast-web:8080/proxy/",
  "fleet": { "workspace": "fleet-default", "gitRepo": "laoban-apps", "bundlePrefix": "laoban-apps-apps-breakfast" }
}
```

- 同一個 `planId` 只能成功 apply 一次（成功後再送回 `409`）；apply 失敗（例如 `ai_invalid_output`）時可以用同一個 `planId` 重送，不必重新產生計畫。
- 回應的 `rollout` 列出兩個 Deployment：`{deployment, path, contentHash}`，`path` 是經 Rancher 代理讀這個 Deployment 的 Steve 路徑，`contentHash` 是它的 Pod 範本該有的 `laoban.io/content-hash`。頁面在 Fleet Bundle 就緒之後，還會確認兩個 Deployment 都已換成這個範本、舊 Pod 都已退場，才顯示「打開應用」（實測曾遇到 Bundle 先報就緒、新 Pod 一分鐘後才起來）。
- 回應多 `mode` 與 `appName`。新應用在 commit 前再確認一次名稱沒被占用。修改現有應用時，產生程式那一步另外帶 `current_code` 資料區塊（目前的四個檔案），要求在保留其餘行為的前提下改出完整的四個檔案。

### `GET /api/apps`

列出 `apps/` 底下的每個應用，最近修改的在前：`{"apps": [{appName, editable, summary, updatedAt, revisions, appUrl, path, fleet}]}`。沒有可讀 `laoban.json` 的目錄（例如較早版本產生的）也會列出，`editable: false`。不含程式內容。

### `GET /api/apps/<appName>`

`{appName, summary, steps, architecture, updatedAt, history, appUrl, path, fleet}`，`history` 新的在前，每筆 `{at, kind: create|edit, request, summary, model}`（文字都已遮蔽）。不存在回 `404 app_not_found`；沒有 `laoban.json` 回 `409 not_editable`。


### `GET /api/apps/<appName>/versions`

每一版就是改到該應用 `laoban.json` 的一個 commit（GitHub commits API），新的在前，最多 50 版：`{appName, versions: [{version, commit, commitUrl, at, kind: create|edit|restore|commit, request, summary, model, current}], truncated}`。需求、摘要、模型取自目前 `laoban.json` 的紀錄（最新的紀錄對應最新的 commit）；對不上時用 commit 訊息。

### `GET /api/apps/<appName>/versions/<commit>`

那一版的 `{appName, commit, summary, steps, architecture, updatedAt}`（讀那個 commit 的 `laoban.json`；不含程式）。`commit` 必須是完整 SHA；找不到回 `404 version_not_found`。

### `POST /api/apps/<appName>/restore`

請求：`{ "commit": "<完整 SHA>" }`。把那一版的計畫與程式當成新的一版再發布（不呼叫 AI、不改寫 Git 歷史，紀錄多一筆 `kind: restore`），回應格式同 `/api/apply`（`mode: "restore"`）。內容與目前版本相同時回 `400`。
### 錯誤

一律 `{"error": {"code": "…", "message": "…"}}`，`message` 是給使用者看的繁中句子。代碼：`bad_request`（400）、`plan_not_found`（404）、`already_applied`（409）、`app_exists`（409）、`app_not_found`（404）、`not_editable`（409）、`version_not_found`（404）、`budget_exceeded`（429，laoban 自己的 token 上限）、`spend_limit`（402，Anthropic 花費上限，**不重試**）、`ai_invalid_output`（502，AI 回應不合格式或碰到禁止的資源）、`github_failed`（502）、`internal`（500）。

### 其他

- `GET /healthz`、`GET /version`（已存在）。
- `GET /api/config` → `{ "model": "…", "targetNamespace": "laoban-dev", "repo": "<owner>/<repo>", "tokenBudget": {"cap": …, "used": …}, "aiTimeoutSeconds": 300 }`，給頁面顯示用，不含任何秘密。

## AI 的兩次呼叫

兩次都經過 `laoban-ai` 的 orchestrator：遮蔽（`Destination::Ai`）、資料區塊邊界、token 預算、錯誤分類與不重試規則。模型 ID 來自設定（環境變數 `LAOBAN_MODEL`），不寫死。

1. **計畫**：輸入使用者需求，要求只回 JSON：`{appName, summary, steps[], components:{web:{modules[]}, api:{modules[], endpoints:[{method, path, purpose}]}}, notes}`。後端把 `components` 轉成上面的 `architecture`。
2. **產生程式**（按確認後）：輸入第 1 步的計畫，要求只回 JSON：`{web: {"index.html": "…", "app.js": "…", "style.css": "…"}, api: {"handler.py": "…"}}`。檔名只能是這四個；任何其他鍵都拒絕（`ai_invalid_output`）。
   - `handler.py` 必須定義 `def handle(method: str, path: str, body: bytes) -> tuple[int, dict]`，只能 import Python 標準函式庫，資料放模組層級的變數（記憶體）。
   - `app.js` 呼叫 API 一律用**相對路徑** `api/<endpoint>`（因為應用是經過 Rancher 代理的子路徑打開的）。

每個檔案上限 64 KiB，合計 192 KiB。

## 產生的應用（固定範本，不由 AI 產生）

寫進 GitHub repo 的 `apps/<appName>/`：

| 檔案 | 內容 |
|---|---|
| `fleet.yaml` | `defaultNamespace: laoban-dev`；`namespace: laoban-dev` |
| `configmap-web.yaml` | ConfigMap `<app>-web-files`：AI 產生的三個網頁檔，加上範本的 `server.py` |
| `configmap-api.yaml` | ConfigMap `<app>-api-code`：AI 的 `handler.py`，加上範本的 `main.py` |
| `web.yaml` | Deployment＋Service `<app>-web`，port 8080 |
| `api.yaml` | Deployment＋Service `<app>-api`，port 8080 |
| `README.md` | 這次的需求、計畫摘要、產生時間、模型 ID（需求先遮蔽再寫入） |
| `laoban.json` | 給 laoban 自己讀回的紀錄：計畫與四個程式檔（格式同 AI 的回應，讀回時走同一套驗證）、最近 20 筆需求紀錄（已遮蔽）。應用清單與編輯都靠它 |
| `.fleetignore` | 讓 Fleet 不把 `laoban.json` 當成 manifest |

- 兩個 Deployment 都用 `registry.suse.com/bci/python:3.13`（**實作時查證 tag 存在**，並在 manifest 用 digest；查不到 digest 就用 tag，在 PR 說明標出來），`command: ["python3", "/app/server.py"]` 或 `main.py`。
- 範本 `server.py`：服務 `/app` 下的靜態檔，並把 `api/*` 轉給 `http://<app>-api:8080/`。標準函式庫實作。
- 範本 `main.py`：`http.server` 包住 `handler.handle()`，JSON 進出，例外回 500。
- 安全設定：`runAsNonRoot`、`runAsUser: 65532`、`readOnlyRootFilesystem: true`、去掉所有 capabilities、`seccompProfile: RuntimeDefault`、`allowPrivilegeEscalation: false`；requests 50m／64Mi，limits 200m／128Mi；`automountServiceAccountToken: false`。
- ConfigMap 內容變動時要觸發重新部署：Deployment 的 pod template 加 annotation `laoban.io/content-hash: <sha256>`。
- 所有物件加 label `laoban.io/app: <appName>`；Pod 範本另加 `app.kubernetes.io/managed-by: laoban`。ConfigMap、Deployment、Service 本身**不寫** `app.kubernetes.io/managed-by`：Fleet 經 Helm 部署，Helm 會把它改成 `Helm`，Fleet 比對 Git 時就永遠判定 Modified、Bundle 不會 Ready（第一次實測遇到）。
- 應用網址：`<基底>/k8s/clusters/c-m-26kbxk5f/api/v1/namespaces/laoban-dev/services/http:<app>-web:8080/proxy/`（經 Rancher 代理，用使用者自己的登入；不需要 Ingress）。

## GitHub

- 目標 repo 由設定決定（`LAOBAN_GITHUB_REPO`，例如 `yansheng133/laoban-apps`），分支 `main`。
- 認證：fine-grained personal access token，只授權那一個 repo 的 Contents 讀寫。從 Kubernetes Secret 讀（環境變數 `LAOBAN_GITHUB_TOKEN` 由 `secretKeyRef` 注入），永不寫進 log。
- 用 Git Data API 一次 commit 整個目錄（建 blobs → tree → commit → 更新 ref），commit message：`laoban: <appName> — <summary 前 60 字>`，結尾加 `Generated-by: laoban`。

## Fleet

由 Sam 事先建立一次（任務書 05），laoban 不建立：

- GitRepo `laoban-apps`，workspace `fleet-default`，repo＝上面的 GitHub repo，`paths: [apps]`，目標叢集 `rke2`。
- `GitRepoRestriction`：`allowedTargetNamespaces: [laoban-dev]`，讓 Fleet 只能部署到這個 namespace。
- namespace `laoban-dev` 事先建立。

## 頁面（Rancher UI extension）

- 一個頁面，三個區塊：左邊對話（輸入需求、送出、顯示計畫摘要與步驟）、右邊 3D 架構圖、下方狀態列。
- 左側選單兩頁：**總覽**（新增應用：上面的對話與 3D 圖）與**應用**（`GET /api/apps` 的清單：名稱、摘要、最後更新、Fleet 狀態，「打開」「編輯／詳細」，右上角「新增應用」回到總覽）。
- 點應用名稱進**應用詳細頁**（`/laoban/c/_/apps/<名稱>`）：摘要、目前的建置步驟、完整修改紀錄、目前的 3D 架構圖、GitHub 程式碼連結與「打開應用」；在「要怎麼修改」輸入後產生修改計畫（`baseApp`），確認後 commit 到同一個目錄，完成後頁面重新讀取這個應用。新增與修改共用同一套計畫／部署／等待流程（`composables/useDeployFlow.ts`）。
- 詳細頁的**版本**清單：每一版的時間、建立／修改／還原、當時的需求與 commit。「查看」把那一版的架構放進 3D 圖，預設與目前版本比較（`utils/graphDiff.ts`：元件以 id、連線以 from/to/kind 對應，標示新增、刪除、修改；模組 id 由名稱產生，所以同名模組跨版本對得上）；「還原到這一版」先在頁面上確認，再呼叫 restore，之後跟部署一樣等新版跑起來。產生修改計畫時，3D 圖也一律與目前版本比較，部署前就看得到會改到哪裡。
- 外觀：每頁右上角可切換**亮色／自動／暗色**與**配色**。亮暗走 Rancher 自己的使用者偏好（`prefs/setTheme`，與 User Preferences 同一個設定，Rancher 跟著換 `<body>` 的 theme class）；管理者用全域設定強制主題時以全域設定為準。配色有「Rancher 預設」與海洋、夕陽、葡萄三組，只在 laoban 頁面內覆寫 Rancher 的 primary／link 變數與 3D 圖的分層顏色，亮暗各一組，對比都在 WCAG AA（4.5:1）以上；選擇記在瀏覽器的 localStorage（換瀏覽器或清除網站資料會回到預設）。
- 3D 圖沿用 `web/viewer-prototype` 的程式（three.js 0.160.0、`graph-diff.mjs`）；M0 只需要顯示與縮放，不需要版本比較與拖曳。
- 「確認並部署」按下後顯示 commit 連結，接著每 5 秒讀 Fleet Bundle（Bundle 只剩 Modified、沒有待部署或錯誤時，也視為應用已在執行，顯示連結並附上 Fleet 的訊息）（`/v1/fleet.cattle.io.bundles`，篩 `laoban.io` 前綴或 bundlePrefix）的 Ready 狀態，Ready 後顯示「打開應用」連結。
- 所有 AI 產生的文字用文字方式顯示，不用 `v-html`。
- 錯誤顯示後端回的繁中 `message`。

## 設定（後端環境變數）

| 變數 | 說明 |
|---|---|
| `LAOBAN_LISTEN_ADDR` | 預設 `0.0.0.0:8080`（已存在） |
| `ANTHROPIC_API_KEY` | 從 Secret 注入；設了 `LAOBAN_AI_BASE_URL` 時可省略 |
| `LAOBAN_MODEL` | 模型 ID（Anthropic 模型 ID，或本機模型在端點上的名稱） |
| `LAOBAN_AI_BASE_URL` | 選用。相容 Anthropic Messages API 的端點（例如 Mac 上的 Ollama），只寫 scheme、主機與 port；不設就是 Anthropic API |
| `LAOBAN_AI_TIMEOUT_SECONDS` | 單次 AI 呼叫上限，30～1800，預設 300 |
| `LAOBAN_TOKEN_CAP` | 每月 token 上限，預設 2,000,000 |
| `LAOBAN_GITHUB_REPO` | `<owner>/<repo>` |
| `LAOBAN_GITHUB_TOKEN` | 從 Secret 注入 |
| `LAOBAN_TARGET_NAMESPACE` | 預設 `laoban-dev` |
| `LAOBAN_CLUSTER_ID` | 預設 `c-m-26kbxk5f`，用來組應用網址 |
