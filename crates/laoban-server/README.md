# laoban-server

laoban 後端的 HTTP 伺服器。只用標準函式庫：每個連線一個執行緒、同時最多 64 個連線、每個回應都 `Connection: close`。收到 SIGTERM／SIGINT 時停止接受新連線，最多等 25 秒讓進行中的請求結束（低於 Kubernetes 預設的 30 秒寬限期）。

## 連接埠與路由

| 連接埠 | 內容 |
|---|---|
| `LAOBAN_LISTEN_ADDR`（預設 8080，純 HTTP） | 只有 `GET /healthz`（probe）與 `GET /version` |
| `LAOBAN_TLS_LISTEN_ADDR`（預設 8443，TLS） | API，以 Kubernetes aggregated API 的形式提供：`/apis/api.laoban.yansheng133.github.io/v1alpha1/...`；只有設了 `LAOBAN_TLS_CERT_FILE`、`LAOBAN_TLS_KEY_FILE` 才開 |

TLS 連接埠要求 client 憑證：由 API server 的 request-header CA 簽發、CN 在允許清單內（啟動時讀 `kube-system/extension-apiserver-authentication`，讀不到就啟動失敗）。通過後才採信 `X-Remote-User`、`X-Remote-Group`，當作呼叫者。路徑與身分的設計見 `docs/m0-design.md` 的「後端 API」與「身分」，模組在 `src/identity.rs`、`src/tls.rs`、`src/kube.rs`。

API 的規則：

- `POST` 必須帶 `Content-Type: application/json`，主體可以用 `Content-Length` 或 chunked 傳輸（Kubernetes API server 轉送時可能改成 chunked），上限 512 KiB；兩者同時出現、或其他傳輸編碼，一律 400。
- 錯誤一律是 `{"error": {"code": "…", "message": "…"}}`，`message` 的語言由 `LAOBAN_LANGUAGE` 決定：`en`（預設，未設定或不認得的值也是）或 `zh-Hant`（見 `crates/laoban-m0/src/lang.rs`）。代碼與狀態碼見 `crates/laoban-m0/README.md`。規格之外的三種情況沿用 `bad_request` 代碼、改用較精確的狀態碼：主體過大 413、方法不對 405（附 `Allow`）、沒有這個 API 404。
- 必要的環境變數沒設定或格式不對時，伺服器照常啟動，`/healthz`、`/version` 正常；API 回 `500 internal`，訊息寫出缺少的變數名稱（不寫值）。啟動 log 也會寫一行 `/api disabled: …`。

## 逾時

- 讀取請求（標頭與主體）時，每次 socket 讀取最多等 5 秒；探測端點的回應寫入也是 5 秒。
- 這些是**每次 socket 操作**的逾時，不是整個請求的期限。`/api/plan`、`/api/apply` 等 AI 與 GitHub 的時候 socket 沒有讀寫，不會觸發；這段時間由各用戶端自己的逾時限制（Anthropic 整個呼叫 300 秒、GitHub 每次呼叫 30 秒）。`/api/*` 的回應寫入逾時是 30 秒。
- 前面的 Rancher 代理與瀏覽器若有更短的逾時，要在那一層調整。

## Log

每個請求一行寫到 stderr：

```
laoban-server: POST /apis/api.laoban.yansheng133.github.io/v1alpha1/namespaces/laoban-dev/plans 200 18342ms user=u-abc12 planId=p-20261013-1a2b3c
```

只有方法、路徑（不含查詢字串，控制字元換成 `?`）、狀態碼、耗時、驗證過的使用者、計畫 ID（apply 時只記符合格式的 ID）與錯誤代碼。**不記錄**請求主體、需求、產生的程式、token 或金鑰。

## 環境變數

`LAOBAN_LISTEN_ADDR`（預設 `0.0.0.0:8080`）、`LAOBAN_TLS_LISTEN_ADDR`（預設 `0.0.0.0:8443`）、`LAOBAN_TLS_CERT_FILE`、`LAOBAN_TLS_KEY_FILE`，其餘見 `crates/laoban-m0/README.md` 的「設定」。`laoban-server --help` 會列出全部。

## 測試

```bash
cargo test -p laoban-server
```

`tests/api.rs` 以模擬 provider 與模擬 GitHub 走真正的 socket：plan → apply → 重複 apply、錯誤格式、請求檢查、花費上限與預算、AI 呼叫超過 socket 逾時仍成功、未設定時的行為，以及實際執行檔的 log 不含金鑰與查詢字串。
