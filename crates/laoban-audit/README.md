# laoban-audit

laoban 的稽核紀錄：把稽核事件遮蔽後格式化成 RFC 5424 syslog 訊息，提供 RFC 5425 的 octet-counting 封框，以及依大小輪替的本機檔案輸出。

這個 crate 不含任何網路或 TLS 程式碼，傳輸層之後另外實作。

## 事件內容

`AuditEvent` 包含：

- 時間戳記：由呼叫端提供（`AuditTimestamp`，UTC、微秒精度），格式化時不讀系統時鐘，輸出可重現
- 操作者：Rancher principal ID（例如 `local://u-abc12`），一律保留、不遮蔽
- 動作：`change.submitted`、`change.approved`、`change.rejected`、`deploy.started`、`deploy.succeeded`、`deploy.failed`、`ai.request`、`ai.response`、`backup.skipped_confirmed`、`settings.changed`
- 結果（outcome）、專案（project）、目標（target，例如層或叢集）、說明文字（detail）、自訂 key/value 欄位

## 遮蔽

`AuditFormatter` 本身持有 `laoban_mask::Masker`，格式化前一律以 `Destination::Audit` 遮蔽所有自由文字（project、target、detail、自訂欄位的值）；只有操作者的 principal ID 不遮蔽。沒有不經遮蔽就能產生紀錄的途徑：`SyslogRecord` 只能由 `AuditFormatter::format` 產生。

- 稽核紀錄保留登入帳號（只有送給 AI 的內容才遮蔽登入帳號），這是 `laoban-mask` 的既定行為
- 每筆紀錄附 `masked="N"`，記錄這筆遮蔽了幾處，不記錄原始值
- `AuditEvent` 的 `Debug` 輸出不印自由文字，避免未遮蔽內容經 debug log 外洩

## 訊息格式（RFC 5424）

```
<PRI>1 TIMESTAMP HOSTNAME APP-NAME PROCID MSGID [SD-ID actor="..." action="..." outcome="..." project="..." target="..." 自訂欄位... masked="N"] MSG
```

範例（合成資料）：

```
<133>1 2026-10-12T03:04:05.000120Z app01.example.test laoban 4242 deploy.succeeded [laoban@32473 actor="local://u-test01" action="deploy.succeeded" outcome="success" project="demo" target="cluster/test-a" change_id="c-0001" masked="0"] deployed web to 192.0.2.10
```

- PRI = facility × 8 + severity。facility 預設 `local0`（16），可設定；若客戶偏好標準的稽核 facility，可改用 `log audit`（13）
- severity 由結果決定：success → Notice (5)、info → Informational (6)、rejected → Warning (4)、failure → Error (3)
- TIMESTAMP：RFC 3339 UTC，固定 6 位小數（RFC 5424 允許的上限）
- MSGID：動作名稱
- HOSTNAME（≤255）、APP-NAME（≤48）、PROCID（≤128）：只能是不含空白的可列印 US-ASCII；建立 `AuditFormatter` 時檢查，不合規直接回傳錯誤，不默默截斷（截斷主機名稱會影響追溯）。空字串輸出 NILVALUE `-`
- SD-ID：預設 `laoban@32473`。**32473 是 RFC 5612 保留給文件範例用的 Private Enterprise Number，正式環境必須換成向 IANA 申請到的 PEN。** SD-ID 可設定，格式須為 `name@數字`，總長 ≤32
- SD-PARAM 值依 RFC 5424 6.3.3 節跳脫 `"`、`\`、`]`；自訂欄位名稱須符合 SD-NAME（≤32、不含 `=`、`]`、`"`、空白），另外不允許 `@`，也不能和內建名稱重複
- MSG（detail）：含非 ASCII 字元時以 UTF-8 輸出並加 BOM（RFC 5424 6.4 節：MSG 以 UTF-8 編碼時必須以 BOM 開頭）；純 ASCII 時視為 MSG-ANY，不加 BOM。SD-PARAM 值依 6.3.3 節是 UTF-8，不加 BOM
- 控制字元（含 CR、LF、Tab）在自由文字與 principal ID 中一律換成 `#` 加三位八進位碼（與 rsyslog 相同，LF 變成 `#012`），確保一筆紀錄只佔一行

## 封框（RFC 5425）

`frame_octet_counting` 產生 `MSG-LEN SP SYSLOG-MSG`，MSG-LEN 是位元組數（不是字元數），供之後的 TLS 傳輸使用。

## 本機檔案與輪替

`FileSink` 以**換行分隔**寫入：每行一筆 RFC 5424 訊息加 LF，不是 octet-counting 封框。要經 TLS 重送時，去掉 LF 再交給 `frame_octet_counting`。

- 寫入前若目前檔案非空、且寫入後會超過 `max_bytes`，先輪替：`audit.log.1` → `audit.log.2` ……、`audit.log` → `audit.log.1`，再開新的 `audit.log`
- `max_files` 是保留的輪替檔數量（不含目前寫入中的檔案）；超過時刪除最舊的一份，所以外部封存作業必須在檔案被輪替掉之前收走
- 一筆紀錄不會被拆到兩個檔案
- 單筆紀錄本身就超過 `max_bytes` 時照樣寫入（寫到一個新的空檔案），不丟棄；下一次寫入時該檔案會被輪替掉
- 新建檔案權限為 0640（Unix）
- 同一個檔案只能由一個 `FileSink` 寫入，不支援多個行程同時寫
- `sync()` 呼叫 `fsync`；輪替前也會先 `fsync`

## 尚未實作

- TODO：每週壓縮輪替檔、保存 6 個月：由外部排程處理（例如 logrotate 或叢集內的 CronJob），不在這個 crate
- TODO：RFC 5425 TLS 傳輸、重送與佇列：之後另外實作
- 尚未申請 IANA Private Enterprise Number，正式上線前要換掉預設 SD-ID
