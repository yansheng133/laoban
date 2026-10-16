# 試吃版（M0）設計

試吃版只做一條最短的路，讓人能實際用用看：**一句需求 → AI 計畫與 3D 架構圖 → 確認 → 程式寫進 GitHub → Fleet 部署 → 打開連結**。這份文件是後端、extension 與部署三邊共用的規格；三邊有衝突時以這裡為準。

## 範圍

| 做 | 不做（延到第一階段） |
|---|---|
| 多人用自己的 Rancher 登入，權限是 Kubernetes RBAC（內建 edit／view，或 Rancher Project 角色「laoban user／viewer」） | 身分模擬、審查流程 |
| 單一目標：rke2 叢集的 namespace `laoban-dev` | 多環境層、正式層、預覽環境 |
| 產生的應用固定兩個元件：網頁＋API | 資料庫、佇列、任意架構 |
| 程式放 ConfigMap，跑在固定的 SUSE BCI Python 映像上 | 映像建置、cosign、SUSE Security 准入 |
| 送 AI 前遮蔽、全域 token 上限、花費上限不重試 | 稽核紀錄外送（只寫後端自己的 log） |
| 介面有繁體中文、英文、日文 | 其他語言 |
| 版本就是 Git commit；還原是把舊版當成新的一版發布；3D 圖拖曳存版 | 版本 CRD |

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

瀏覽器呼叫後端走 Rancher 的 `/k8s/clusters/<id>` 代理，用的是使用者自己的登入；後端是 rke2 上的 **Kubernetes aggregated API**，由 API server 驗證使用者與權限後才轉給後端（見「身分」）。後端只寫入自己在 `laoban-system` 的狀態資源（意見收斂的 `Round`，見「保存與重啟」），應用的部署全由 Fleet 做。

## 後端 API

基底路徑：`/k8s/clusters/<叢集 ID>/apis/api.laoban.yansheng133.github.io/v1alpha1/namespaces/<目標 namespace>`（Sam 的環境是 `c-m-26kbxk5f`、`laoban-dev`）。頁面在執行時找出叢集與 namespace（v0.1.17，見「頁面找後端」）。全部 JSON，UTF-8。下面各節沿用內部的 `/api/...` 名稱，對應的外部路徑（接在基底路徑後）如下，括號是 RBAC 上的資源與動作：

| 內部名稱 | 外部路徑 |
|---|---|
| `GET /api/config` | `GET /config`（`config`, list） |
| `POST /api/plan` | `POST /plans`（`plans`, create） |
| `POST /api/apply` `{planId}` | `POST /plans/<planId>/apply`（`plans/apply`, create），不需要主體 |
| `GET /api/apps`、`/api/apps/<n>[/...]` | `GET /apps`、`/apps/<n>[/...]`（`apps`, list／get；子資源 `apps/versions`、`apps/files`…） |
| `POST /api/apps/<n>/<動作>` | `POST /apps/<n>/<動作>`（`apps/<動作>`, create） |

### 身分

- 後端以 `APIService` `v1alpha1.api.laoban.yansheng133.github.io` 掛在 rke2 的 API server 底下（chart 的 `apiservice.yaml`）。Rancher 代理的請求到 API server 時已是登入的 Rancher 使用者（Rancher 以 impersonation 代為發出），API server 驗證身分、用這個使用者的 RBAC 授權（以 namespace 為範圍，所以跟著 Rancher Project 走），再以自己的 proxy client 憑證透過 TLS 轉給後端，使用者放在 `X-Remote-User`、`X-Remote-Group`。
- 後端只在 TLS 連線、且 client 憑證由 request-header CA 簽發、CN 在允許清單內時才採信這些 header；CA、允許的名稱與 header 名稱於啟動時從 `kube-system/extension-apiserver-authentication` 讀取（後端 ServiceAccount 唯一的權限：內建 Role `extension-apiserver-authentication-reader`）。其他人（例如直接連 Service 的 Pod）拿不出這張憑證，無法冒充別人。
- 8080 port 只提供 `/healthz`、`/version`；API 只在 8443（TLS）。
- 有請求本體的寫入要以 JSON 送出（`Content-Type: application/json`），用來擋跨站的表單送出。經 API server 轉來、身分已驗證的請求可以不帶 `Content-Type`（`kubectl create --raw` 不會送）；帶了其他類型照樣回 400。（v0.1.7，任務書 13 的 B6）
- 使用者權限：chart 的 `laoban-api-edit`／`laoban-api-view` 帶有標準聚合標籤，所以在 `laoban-dev` 有內建 edit／admin 的人可以使用、有 view 的人可以看。Rancher 管理者有全部權限。Rancher Project 成員要用 laoban，在 Rancher local 叢集套用 `deploy/rancher/roletemplate.yaml`（v0.1.16），把成員設成 Project 角色「laoban user」或「laoban viewer」；規則與上面兩個 ClusterRole 相同，CI 會比對（見 `deploy/rancher/README.md`）。沒有權限時 API server 回 403，頁面顯示「你沒有這個操作的權限…」。
- 使用者名稱是 API server 給的名稱，Rancher 使用者是 Rancher 的 user ID（例如 `u-abc12`）。這個名稱記進每一版的紀錄（`history[].by`、版本清單的 `by`）與後端 log（`user=`）。
- 文件：https://kubernetes.io/docs/tasks/extend-kubernetes/configure-aggregation-layer/

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
- 計畫存在後端記憶體（以 `planId` 為鍵，保留 1 小時，最多 20 筆）。後端重啟就消失，新增應用時可以接受；意見收斂的計畫另外跟著那一輪保存（見「保存與重啟」）。

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

`{appName, summary, steps, architecture, updatedAt, history, appUrl, path, fleet, round}`，`history` 新的在前，每筆 `{at, kind: create|edit|restore|manual|layout, request, summary, model}`（文字都已遮蔽）。`architecture` 的位置已套用存過的版面；`round` 是這個應用最近一輪的意見收斂（見下）或 `null`。`GET /api/apps` 每筆多一個 `round`：進行中那一輪的狀態，沒有就是 `null`。不存在回 `404 app_not_found`；沒有 `laoban.json` 回 `409 not_editable`。


### `GET /api/apps/<appName>/versions`

每一版就是改到該應用 `laoban.json` 的一個 commit（GitHub commits API），新的在前，最多 50 版：`{appName, versions: [{version, commit, commitUrl, at, kind: create|edit|restore|manual|layout|commit, request, summary, model, current}], truncated}`。需求、摘要、模型取自目前 `laoban.json` 的紀錄（最新的紀錄對應最新的 commit）；對不上時用 commit 訊息。

### `GET /api/apps/<appName>/versions/<commit>`

那一版的 `{appName, commit, summary, steps, architecture, updatedAt}`（讀那個 commit 的 `laoban.json`；不含程式）。`commit` 必須是完整 SHA；找不到回 `404 version_not_found`。

### `POST /api/apps/<appName>/restore`

請求：`{ "commit": "<完整 SHA>" }`。把那一版的計畫與程式當成新的一版再發布（不呼叫 AI、不改寫 Git 歷史，紀錄多一筆 `kind: restore`），回應格式同 `/api/apply`（`mode: "restore"`）。內容與目前版本相同時回 `400`。版面也一起還原。

### 意見收斂：`/api/apps/<appName>/round[/...]`

取代原本的編輯鎖（需求「同一 Project 編輯鎖」；Sam 2026-10-14 決定：大家把意見提上來，由 AI 收斂後跟全部人確認，然後 AI 開工）。一個應用同一時間只有一輪；不同應用互不影響。使用者都是 API server 驗證過的名稱（`rounds.rs`）。

| 請求 | 誰 | 做什麼 |
|---|---|---|
| `GET .../round` | 任何人 | `{round, roundsAvailable, limits}`：最近一輪（含已完成或取消的）或 `null`；能不能使用意見收斂（叢集裡有沒有 Round CRD）；開新一輪的大小限制 |
| `POST .../round` `{opinion, items, deadlineMinutes, scope}` | 任何人 | 開一輪。`deadlineMinutes` 0～1440；0 表示馬上開始收斂（回應的狀態是 `converging`，AI 在背景收斂）。`scope` 是 `web`、`api` 或 `all`（預設）。已有進行中的一輪回 `409 round_active` |
| `POST .../round/opinions` `{text, target}` 或 `{items: [{text, target}]}` | 任何人 | 收集中提意見。大小限制見下面的「大小限制」 |
| `POST .../round/opinions/<id>/support` | 意見作者以外的人 | 附議（+1）或取消附議。收集意見與等待確認時都可以 |
| `POST .../round/opinions/<id>/replies` `{text}` | 任何人 | 在這則意見底下回覆。收集意見與等待確認時都可以 |
| `POST .../round/confirm` | 這一輪提過意見的人 | 確認。最後一個回覆時，狀態變成 `applying`，AI 在背景寫程式並發布；完成後狀態是 `done`，`result` 是發布結果（格式同 `/api/apply`） |
| `POST .../round/object` `{text}` | 同上 | 有意見：這段話加進意見，回到收集，10 分鐘後重新收斂 |
| `POST .../round/skip` `{user}` | 發起人 | 略過還沒回覆的人（記錄誰、何時略過）；略過後如果全部都回覆了，同樣開始發布 |
| `POST .../round/cancel` | 發起人 | 取消（收集中、等待確認或失敗時） |
| `POST .../round/converge` | 發起人 | 收斂或發布失敗後重新收斂 |

**AI 在背景執行**（v0.1.14，18 報告 B8）：後端是 aggregated API，經過 Kubernetes API server 的請求約 60 秒就會被切斷，而 AI 寫程式加發布可能超過（實測 114 秒）。所以收斂（開一輪時 0 分鐘、`converge`）和發布（最後一個確認或略過）都在後端自己的執行緒做，請求馬上回應目前的狀態（`converging` 或 `applying`）；頁面每 5 秒讀一次這一輪，看到 `done` 就跟一般部署一樣等新版跑起來。頁面上顯示的秒數依這一輪的紀錄計算，所有人看到的一樣。萬一請求還是沒有回應（逾時、網路中斷、API server 的錯誤），頁面先讀一次這一輪：狀態已經往前走，就顯示「後端繼續進行」而不是錯誤。

新增應用與一般的計畫／部署也一樣：`POST /api/plan` 或 `.../plans/<planId>/apply` 帶 `"background": true` 時，回應是一項工作 `{jobId, kind, state: "running"}`，頁面每 2 秒讀 `GET .../plans/<jobId>`（`plans` 的 get，edit 角色本來就有），直到 `done`（`result` 就是原本的回應）或 `failed`（`error` 是原本會回的錯誤：status、code、message）。工作只有發起的人讀得到，保留 1 小時，重啟後消失（頁面會說找不到進度，請到應用清單確認）。不帶 `background` 時照舊在請求裡完成。

**錯誤格式**（v0.1.14）：錯誤回應同時是 Kubernetes 的 `Status`（`kind`、`status: Failure`、`message`、`reason`、`code`），`kubectl create --raw` 會印出中文原因；頁面照舊讀 `error.code`／`error.message`，而且以它為準（不會把後端的 404、403 當成 API server 的「連不上」「沒權限」）。

**版本的檔案指紋**（v0.1.14）：`GET .../apps/<name>` 與 `.../versions/<commit>` 多一個 `fileDigests`：四個程式檔各自 sha256 的前 16 個十六進位字。頁面比較兩個版本時，用它列出改了哪些程式檔，所以只改程式、沒改架構的版本也看得出改了什麼。

**意見編號**（v0.1.14）：頁面上每則意見標 #1、#2…（意見 ID 去掉 o）；交給 AI 的意見也用這個編號，並請 AI 在說明裡引用「#2「幾個字」」，不用內部 ID。

**針對元件的意見**（v0.1.11）：
- 每則意見有 `id`（`o1`、`o2`…），可以帶 `target`。`target` 是目前架構裡的節點 id（3D 圖用的那個），後端會補上元件名稱；不在架構裡的節點回 400。
- `items` 一次最多 10 則，全部檢查通過才一起加入。開一輪時，`opinion` 和 `items` 可以擇一或同時給。
- 每則意見有 `supporters`（附議者，不含作者）與 `replies`（每則最多 20 個回覆，一則回覆最多 300 字）。回覆的字數算進這一輪的 8000 字上限。
- 要確認的人仍然只有寫意見的人；附議、回覆的人不必確認。
- 等待確認時的附議與回覆只是討論，不會改變已經收斂好的計畫。
- 收斂時，意見依元件分組交給 AI：先放整個應用的意見，再按元件分組，每則附上作者、附議者與回覆。AI 會被告知附議越多的意見越多人同意。

狀態：`collecting` → `converging` → `confirming` → `applying` → `done`；也可能是 `failed`、`cancelled`。每一輪的 `you` 寫出呼叫者是不是發起人、要不要確認、能不能提意見；`log` 記錄每個動作與時間：每筆有繁中的 `event`，以及給頁面依介面語言顯示的 `code`（`opened`、`opinion`、`objected`、`converging`、`converged`、`failed`、`confirmed`、`skipped`、`applying`、`done`、`cancelled`、`restarted`）與 `args`（`minutes`、`count`、`user`、`message`）。

- **收斂**：截止時間到了，後端（每 10 秒檢查一次）把所有意見連同作者交給 AI，用修改既有應用的計畫流程，另外指示 AI 合併重複的意見、衝突時採較小較安全的改動並在 notes 說明；不需要修改或做不到時，也要在 notes 說明原因。計畫的 `notes` 會顯示在頁面上。收斂後的計畫實際動到的部分（網頁模組、API 模組或端點）會加進這一輪的範圍。範圍是依計畫的模組判斷的，AI 實際改寫的檔案可能超出範圍；超出範圍的手動修改由下面的保留檢查保護。
- **確認**：要確認的人是這一輪提過意見的人。收斂好的計畫跟著這一輪保存，不會因為等太久或後端重啟而失效（v0.1.15 起；之前是 1 小時）。
- **一輪進行中（收集、收斂、等待確認、發布、失敗）時**：
  - 另外產生修改計畫（有 `baseApp` 的 `/api/plan`、套用修改計畫）與還原，回 `409 round_active`。
  - 直接存程式時，改到範圍內的檔案回 `409 round_scope`，範圍外照常存。網頁範圍是 `index.html`、`app.js`、`style.css`；API 範圍是 `handler.py`。
  - 存版面不受影響（只改位置，不碰程式）。
- **發布時逐檔合併**：AI 依計畫產生程式後，跟「現在的版本」逐檔比較。只有一邊改過的檔案取那一邊；AI 和手動都改了、內容又不同的檔案是衝突，回 `409 merge_conflict`，這一輪變成失敗，不發布。修改紀錄與版面也用現在的版本，期間的手動修改與版面調整都會留下。這個合併規則也適用於一般的修改計畫。
- **保留這一輪期間的手動修改**：後端記下一輪開始時應用的 commit（v0.1.15 起；之前是記下四個檔的內容），收斂時從 GitHub 讀回那時的四個檔。收斂時，如果有檔案在這之後被手動改過，產生程式時會把手動加入與刪除的行（data block `manual_edits`）交給 AI，並要求保留。AI 產生的程式如果改了這個檔，卻少了手動加入的行、或帶回手動刪除的行，就回 `409 merge_conflict`，這一輪變成失敗，不發布；發起人可以重新收斂。比對以去掉前後空白的整行為單位，忽略空行。（v0.1.7，任務書 13 的 B7）
- **修改紀錄**：一輪發布的版本，紀錄裡的「需求」是意見清單「（使用者）意見；…」，不是送給 AI 的完整指示。
**v0.1.16 新增**：

| 請求 | 誰 | 做什麼 |
|---|---|---|
| `POST .../round/opinions/<id>/edit` `{text}` | 意見作者 | 收集中改寫自己的意見（字數限制同提意見）。已有人附議時保留附議，意見標 `editedAfterSupport` |
| `POST .../round/opinions/<id>/withdraw` | 意見作者 | 收集中撤回：原文、它的回覆與附議一起刪除（任何人都看不到，也不交給 AI），編號保留，標 `withdrawn`；不能再附議或回覆。撤回的也算進一輪最多意見數的兩倍（60 則），作者不再是確認人。所有意見都撤回時，截止後這一輪自動取消（紀錄 `emptyCancelled`），不呼叫 AI |
| `POST .../round/deadline` `{minutes}` | 發起人 | 收集中把截止時間改成現在加幾分鐘；超過開始後 24 小時的改成剛好 24 小時（v0.1.17，之前會被拒絕）；0 表示立即收斂。一輪的 JSON 有 `latestDeadline`，頁面只列出它之前的選項 |
| `POST .../round/drafts` `{items: [{text, target}], shared}` | 任何人 | 收集中把自己的草稿（針對元件的與一般的）存在這一輪裡，取代先前存的；`shared` 預設 `true`（其他人看得到，不能改）。每人最多 10 則、每則 1000 字，全部草稿合計不超過一輪的 8000 字；不算進意見、不進紀錄、不寫進歸檔。頁面在最後一次變更 1.5 秒後儲存；還沒有一輪時草稿只在瀏覽器裡 |
| `POST .../round/plan/steps/<n>/comments` `{text}` | 任何人 | 等待確認時針對計畫第 n 步留言（300 字，最多 50 則，算進一輪的 8000 字）。歸檔時留言也遮蔽 |
| `POST .../round/plan/comments/<id>/support` | 留言者以外 | 附議留言或取消 |
| `GET .../waitingrounds`（RBAC 資源 `waitingrounds`，動作 list；v0.1.17 前是 `rounds/mine`，跟 Round CRD 撞名） | 任何人 | 所有應用中等呼叫者處理的輪次：`confirm`（等他確認）、`othersConfirm`（他發起或已確認、還有人沒回覆）、`retry`（他發起、失敗）、`failed`（他確認過、失敗）。只讀記憶體 |
| `GET .../apps/<name>/rounds`、`.../rounds/<id>`（RBAC 子資源 `apps/rounds`） | 任何人 | 從 `laoban-rounds` 分支讀這個應用歸檔的輪次：清單（依輪次 ID 由新到舊，最多 50 輪；v0.1.17 起 ID 是 `r-YYYYMMDD-` 加當天第幾秒（5 位十六進位）與 1 位亂數，同一天也依時間排）與單一輪（遮蔽過的內容、`openedAt`、`archivedAt`） |

- 一輪的 JSON 多了 `drafts`（`mine`：自己的；`shared`：其他人分享的）與 `planComments`；`you.canComment` 表示能不能對計畫留言。對計畫的留言在有人按「有意見」後跟著意見一起交給 AI（標出是哪一步、原本的步驟文字、附議），新的計畫產生後清空。留言不影響確認，確認的人仍然只有寫意見的人。
- 紀錄新增 `edited`、`withdrawn`、`deadline`（`minutes`，0 是立即收斂）、`commented`（`step`）、`emptyCancelled`。
- 失敗原因多了 `messageCode`／`messageArgs`（例如 `restartUnpublished`、`mergeConflict` 帶 `files`），頁面依語言顯示；沒有代碼的照舊顯示中文 `message`。發布完成的紀錄改成「新版本已寫入 GitHub，等待 Fleet 部署」。
- 所有應用共用一個分支，laoban 的 commit 一次一個；同一個應用的版本一次只寫一個（手動存檔、版面、還原、發布），第二個請求等第一個寫完，再和它寫的版本比對，所以會收到 `409 version_conflict`（「有了新版本」）而不是 GitHub 錯誤。萬一分支被別人（不經過 laoban）先推了，也回 `409 version_conflict`（20 報告 B11）。
- 頁面：應用清單標出等你確認、等其他人確認、失敗待處理的應用，上方寫出幾輪等你確認；左側選單的「應用」顯示等你確認的件數（每 60 秒更新）；詳細頁的意見收斂上方顯示「這一輪等你確認」。詳細頁有「過去的輪次」，打開才讀取。

**v0.1.17**：

- **頁面找後端**：extension 不再寫死叢集 ID 與 namespace。第一次需要時，先試上次記住的（這個瀏覽器），再列出使用者看得到的叢集（`/v1/management.cattle.io.clusters`），對每個叢集讀 laoban API group 的 discovery（任何登入的人都能讀）；在有的那個叢集用 `GET .../namespaces/<ns>/config` 找 namespace：先試 `laoban-dev`，再試使用者在那裡看得到的 namespace。後端對其他 namespace 的請求回 `404 other_namespace`，`error.targetNamespace` 寫出它服務的 namespace（能在某個 namespace 呼叫這個 API 的人就能知道它的名稱），所以通常一個請求就找到。找不到時頁面說明要請管理者給角色，或照 Quickstart 安裝。一個 Rancher 只用第一個找到的後端。
- **可以修改嗎**：頁面用 SelfSubjectAccessReview 問 API server 能不能 `create` `apps/files`；不能的人看不到新增應用與編輯按鈕（後端照樣檢查）。
- **背景工作的 log**：收斂、發布、產生計畫在背景失敗時，後端 log 寫一行 `<工作> <應用> <輪次> failed: <代碼> (<原因>)`，原因同同步請求的 `detail`，不含 token、金鑰與使用者的文字。
- **Rancher 側邊選單**：laoban 的頁面可以收合 Rancher 的側邊選單。做法是在 `<body>` 加 class，把 Rancher 版面的 `--nav-width` 設成 0 並隱藏選單；只在 laoban 的頁面上，離開就恢復；選擇記在瀏覽器。
- **發布**：打 `vX.Y.Z` tag 後，`publish` workflow 用 `@rancher/shell` 的發布腳本建 extension 與它的 chart，加上兩個後端 chart（`laoban` 的 `image.digest` 填這個版本的映像檔），寫進 `gh-pages` 分支，成為 Helm repo（`https://yansheng133.github.io/laoban`），並以 `docs/releases/<tag>.md` 建立 GitHub Release。tag、Cargo、兩個 chart 與 extension 的版本必須一致。

**保存與重啟**（v0.1.15）：
- 每一輪存成一個 `Round` 資源（`laoban-crd` chart，group `laoban.yansheng133.github.io/v1alpha1`），放在後端自己的 namespace `laoban-system`，名稱 `<應用>-<輪次 ID>`，label `laoban.yansheng133.github.io/app`。`spec` 有 `schema`（目前 1）、`app`、`targetNamespace`、`state`、`deadline` 與 `round`（這一輪的全部內容，包括頁面看不到的開始時的 commit、收斂好的計畫要寫程式時需要的內容）。
- 後端的權限只多一個 Role：在 `laoban-system` 對 `rounds` 做 get、list、create、update、delete。使用者沒有這個資源的權限，只能經過後端的 API。
- 每個動作（開一輪、提意見、附議、回覆、確認、略過、有意見、取消、重新收斂）都先寫進資源，寫成功才回應頁面；寫入失敗時這一輪不變，回 `503 state_failed`。寫入帶著讀到的 `resourceVersion`，有別人先寫了（另一個 replica 或有人用 kubectl 改過）就重新讀取，在最新的內容上再做一次這個動作（`409 round_conflict` 只在重試後仍衝突時出現）。後端自己的步驟（收斂中、等待確認、發布中、完成、失敗）也都會寫入；這些步驟寫入失敗時只記在後端的 log，重啟後從最後寫入的狀態接著做。
- 後端啟動時讀回所有輪次，倒數照常進行。重啟時正在**收斂中**的，AI 的工作已經中斷，自動重新收斂（紀錄多一筆 `restarted`）。正在**發布中**的，先看應用最新的修改紀錄：已經有這一輪的新版本就標成完成，沒有就標成失敗，請發起人重新收斂。不會在不確定的情況下重新發布，免得發布兩次。
- 叢集裡沒有 Round CRD 時，後端照常啟動，其他功能都能用；意見收斂回 `503 rounds_unavailable`，頁面顯示「請管理者先升級 laoban-crd chart」。後端每 10 秒再試一次，CRD 裝好就能用。
- **歸檔**：一輪完成或取消後，後端在背景把整輪內容寫進應用 repo 的 `laoban-rounds` 分支，路徑 `rounds/<應用>/<輪次 ID>.json`。這個分支第一次建立時沒有任何歷史（跟 `main` 無關），Fleet 只追蹤 `main`，所以不會觸發部署。意見與回覆的文字和寫進 GitHub 的其他內容一樣先遮蔽；頁面看不到的欄位不寫。寫入失敗會在 10 秒後重試，之後間隔加倍，最多 10 分鐘；歸檔成功前資源不刪，後端重啟後接著重試。最近一輪保留在叢集裡，直到這個應用的下一輪開始，那時已歸檔的上一輪才刪除。
- 新增應用的背景工作（產生計畫、部署）仍然只在記憶體，重啟後頁面會請使用者到應用清單確認結果。

**大小限制**（v0.1.15）：

| 項目 | 上限 |
|---|---|
| 一則意見（含「有意見」） | 1,000 字 |
| 一則回覆 | 300 字 |
| 一輪的意見 | 30 則（一次送出最多 10 則） |
| 一則意見的回覆 | 20 則 |
| 一輪所有意見加回覆 | 8,000 字（整份會交給 AI） |
| AI 收斂出來的計畫（連同寫程式需要的內容） | 150 KB；超過時這一輪失敗，請發起人縮小範圍 |
| 紀錄 | 保留最近 100 筆 |
| 一個 `Round` 資源 | 512 KB（Kubernetes 單一物件約 1.5 MB；照上面的限制實際遠小於此） |

- 字數以 Unicode 字元計（一個中文字、一個 emoji 都算一個字），不含前後空白。
- 單則太長回 `400 too_long`，這一輪滿了回 `400 round_full`，訊息寫出上限的數字。
- 管理者可以用 chart 的 `rounds.limits` 把前五項調低（環境變數 `LAOBAN_ROUND_MAX_OPINION_CHARS`、`LAOBAN_ROUND_MAX_REPLY_CHARS`、`LAOBAN_ROUND_MAX_CHARS`、`LAOBAN_ROUND_MAX_OPINIONS`、`LAOBAN_ROUND_MAX_REPLIES`），不能調高：設定值高於上面的數字時 chart render 失敗，後端也不會啟動 API。限制對所有人都一樣，管理者也不例外。
- 頁面從後端讀取限制，送出前先檢查：輸入框下有字數，到 90% 變色；超過時文字照留（不截斷，以免沒發現內容被切掉），超出的部分以紅框標示，送出按鈕停用並說明要刪掉幾個字。這一輪用到 70% 時顯示還剩多少；滿了就不能再提意見或回覆，附議不受影響。

**之後的共同編輯（暫存檔，尚未實作）**：儲存的程式（`crates/laoban-m0/src/state.rs`）是共用的一層，之後的暫存檔沿用同樣的寫入方式與版本比對：兩個人同時存時，後存的人不會蓋掉前一個人的內容，而是收到「別人剛改過」。初步方向：
- 程式碼：每個應用一份共用的暫存，存成新版本時才 commit 到 GitHub。
- 意見草稿：放在那一輪的 `Round` 裡，一輪結束就沒有用。
- AI 收斂後的計畫：大家一起修改或 review，修改記在那一輪的計畫上，記下誰改了哪裡。
- 待討論：3D 版面與改結構的共同編輯；同時打字、看到別人游標的即時共同編輯。

### `GET /api/apps/<appName>/files`、`POST /api/apps/<appName>/files`

`GET` 回 `{appName, commit, files}`：目前的四個檔（格式同 AI 產生程式的回應）與最新版本的 commit，給網頁編輯器用。

`POST` 請求 `{ "baseCommit": "<GET 拿到的 commit>", "files": {…}, "note": "這次改了什麼（選填）" }`：把手動修改的程式存成新的一版（`kind: manual`，commit 訊息「手動修改：…」），**不呼叫 AI**，計畫與版面不變。檔案要通過與 AI 產生的程式相同的檢查（只能是那四個檔、每檔 64 KiB、合計 192 KiB、`handler.py` 要定義 `handle(...)`），不通過回 `400 invalid_files`；與目前內容相同回 `400`；`baseCommit` 不是最新版本（編輯期間有人存了新版）回 `409 version_conflict`。請求本體上限因此提高到 512 KiB。

### `POST /api/apps/<appName>/layout`

請求 `{ "baseCommit": "…", "positions": { "<node id>": {"x": …, "z": …} } }`：3D 圖拖曳後的位置存成新的一版（`kind: layout`，需求「調整 3D 版面」），**不呼叫 AI**，程式不變。node id 必須是目前架構裡的元件，座標絕對值 ≤ 100，最多 100 筆；與既有版面合併。位置存在 `laoban.json` 的 `layout`（只有拖過才有），之後的修改、手動修改都會帶著；還原時用那一版的版面。修改計畫改名了某個模組時（模組 id 跟著名稱，所以看起來是一個刪除、一個新增），同一個元件裡剛好一個有位置的模組消失、一個新模組出現，新模組就沿用舊位置；已不存在的元件的位置會被丟掉。

### 錯誤

一律 `{"error": {"code": "…", "message": "…"}}`，`message` 是給使用者看的繁中句子。代碼：`bad_request`（400）、`round_not_found`（404，沒有這個歸檔的輪次）、`too_long`（400，意見或回覆太長）、`round_full`（400，這一輪的意見已滿）、`rounds_unavailable`（503，叢集裡沒有 Round CRD）、`round_conflict`（409）、`state_failed`（503，寫入 Round 資源失敗）、`plan_not_found`（404）、`already_applied`（409）、`app_exists`（409）、`app_not_found`（404）、`not_editable`（409）、`version_not_found`（404）、`unauthenticated`（401，請求沒有經過 API server、沒有驗證過的使用者）、`round_active`（409）、`round_scope`（409）、`merge_conflict`（409）、`round_state`（409，這一輪目前的狀態不能這樣做）、`not_initiator`（403）、`not_confirmer`（409）、`version_conflict`（409）、`invalid_files`（400）、`budget_exceeded`（429，laoban 自己的 token 上限）、`spend_limit`（402，Anthropic 花費上限，**不重試**）、`ai_invalid_output`（502，AI 回應不合格式或碰到禁止的資源）、`github_failed`（502）、`internal`（500）。

### 其他

- `GET /healthz`、`GET /version`（已存在）。
- `GET /api/config` → `{ "model": "…", "targetNamespace": "laoban-dev", "repo": "<owner>/<repo>", "tokenBudget": {"cap": …, "used": …}, "aiTimeoutSeconds": 300, "roundLimits": {…}, "roundsAvailable": true }`，給頁面顯示用，不含任何秘密。

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
- 完成或取消的意見收斂歸檔到同一個 repo 的 `laoban-rounds` 分支（沒有歷史的獨立分支，第一次歸檔時建立；token 權限不變，仍是 Contents 讀寫）。`main` 只放應用。

## Fleet

由 Sam 事先建立一次（任務書 05），laoban 不建立：

- GitRepo `laoban-apps`，workspace `fleet-default`，repo＝上面的 GitHub repo，`paths: [apps]`，目標叢集 `rke2`。
- `GitRepoRestriction`：`allowedTargetNamespaces: [laoban-dev]`，讓 Fleet 只能部署到這個 namespace。
- namespace `laoban-dev` 事先建立。

## 頁面（Rancher UI extension）

- 一個頁面，三個區塊：左邊對話（輸入需求、送出、顯示計畫摘要與步驟）、右邊 3D 架構圖、下方狀態列。
- 左側選單兩頁：**總覽**（新增應用：上面的對話與 3D 圖）與**應用**（`GET /api/apps` 的清單：名稱、摘要、最後更新、Fleet 狀態，「打開」「編輯／詳細」，右上角「新增應用」回到總覽）。
- 點應用名稱進**應用詳細頁**（`/laoban/c/_/apps/<名稱>`）：摘要、目前的建置步驟、完整修改紀錄、目前的 3D 架構圖、GitHub 程式碼連結與「打開應用」；在「要怎麼修改」輸入後產生修改計畫（`baseApp`），確認後 commit 到同一個目錄，完成後頁面重新讀取這個應用。新增與修改共用同一套計畫／部署／等待流程（`composables/useDeployFlow.ts`）。
- 詳細頁的**版本**清單：每一版的時間、建立／修改／還原、當時的需求與 commit。「查看」把那一版的架構放進 3D 圖，比較對象可選**前一版**（預設，標示那一版自己改了什麼）、**目前版本**（標示還原後會改什麼）或**不比較**（`utils/graphDiff.ts`：元件以 id、連線以 from/to/kind 對應，標示新增、刪除、修改；模組 id 由名稱產生，所以同名模組跨版本對得上）；「還原到這一版」先在頁面上確認，再呼叫 restore，之後跟部署一樣等新版跑起來。產生修改計畫時，3D 圖也一律與目前版本比較，部署前就看得到會改到哪裡。
- 3D 圖配色：3D 圖右上角的 🎨 選「Rancher 預設」、海洋、夕陽、葡萄四組，只影響 3D 圖（分層顏色與選取色），頁面本身用 Rancher 的顏色；亮暗跟著 Rancher 自己的使用者偏好（User Preferences），laoban 頁面不另外提供切換。每組配色有自己的選取色，和新增、刪除、修改三種標示顏色分開（`utils/__tests__/palettes.test.ts` 直接讀樣式檔檢查）；選擇記在瀏覽器的 localStorage。
- 3D 圖選取元件：被選取的元件外面加一圈選取色的光圈，名稱標籤改成選取色的粗框；它的連線改成選取色；和它沒有連線、也不是它的上下層（應用與模組）的元件與連線變淡。從變更清單或意見清單點元件名稱也一樣。3D 圖右上角另有「重設視角」。
- 3D 圖沿用 `web/viewer-prototype` 的程式（three.js 0.160.0、`graph-diff.mjs`）。可以顯示七類元件：程式（方塊）、網路服務（圓柱）、資料庫（高圓柱）、Ingress（八面體）、外部相依服務（十二面體）、儲存空間（扁六角柱）、訊息佇列（膠囊）；M0 產生的應用只會用到其中幾類。
- 詳細頁左上是**意見收斂**（`components/RoundPanel.vue`、`composables/useRound.ts`）：沒有進行中的一輪時，填意見、選收斂時間（立即、10 分鐘～24 小時）與範圍後開始；進行中時顯示意見、截止時間、收斂後的計畫與每個人的確認狀態，並依呼叫者的身分顯示能做的動作。頁面每 5 秒更新一次。收斂後的計畫在 3D 圖與目前版本比較。一輪發布後，頁面跟一般部署一樣等新版跑起來。「應用」清單會標出正在進行一輪的應用。
- 詳細頁的 3D 圖高度依視窗調整（320～520px，第一個畫面看得到整張圖）；滾輪捲動頁面，Ctrl（Mac 是 ⌘）加滾輪或觸控板雙指才縮放。3D 圖右上的「放大」讓 3D 圖佔滿瀏覽器視窗（同一個元件，選取、比較與還沒存的位置都延續），可以再切到全螢幕；右側是「版本／變更／意見」分頁：版本時間軸點一下切換版本並自動切到「變更」；變更清單列出和比較對象相比新增、修改、刪除的元件與連線，點一下在圖上選取；模組名稱不是檔名時，附上它在哪個程式檔裡。放大畫面裡目前版本也和前一版比較，看得到最新一次改了什麼。架構沒變、只改程式內容的版本，變更清單會說明架構沒有變動。Esc 或「關閉放大」回到原頁面。有人發布新版時，提示固定在畫面右下角。
- 針對元件的意見：在 3D 圖點一個元件，下方出現「對「元件」提意見」，寫好後「加入草稿」；每個元件一則草稿，可以對好幾個元件各寫一則，再和一般意見一起送出（開一輪或收集期間）。進行中的一輪，3D 圖上有意見的元件旁邊顯示「💬 則數」。意見收斂的清單依元件分組，點元件名稱會在 3D 圖選取它；每則意見可以 +1 和回覆。放大畫面的側欄也有意見清單。
- 一輪進行中時：程式碼分頁裡範圍內的檔案只能看，範圍外照常可以存；版本不能還原；「改結構」只在收集意見時可用。
- 詳細頁右邊分**架構圖**與**程式碼**兩個分頁。程式碼分頁用 Rancher 內建的 CodeMirror 編輯四個檔，「儲存成新版本」走 `POST .../files`，之後跟部署一樣等新版跑起來。
- 架構圖在**最新版本**（沒有在看舊版、沒有修改計畫）時可以拖曳元件，「儲存版面」走 `POST .../layout`。點選元件後可在「改結構」改名、刪除、新增連線，或新增模組：這些會變成句子加進「要怎麼修改」，再走一般的 AI 計畫／確認流程。新增資料庫、訊息佇列、Ingress 不在 M0 的固定範本內，按鈕顯示「試吃版不支援」。歷史版本唯讀。
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
| `LAOBAN_STATE_NAMESPACE` | 選用。存 `Round` 資源的 namespace，預設是 Pod 自己的（`laoban-system`） |
| `LAOBAN_ROUND_MAX_*` | 選用。調低意見收斂的大小限制（見「大小限制」），只能調低 |
