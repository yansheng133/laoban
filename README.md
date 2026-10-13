# laoban

laoban（老闆）是一個 Rancher UI extension：使用者用自然語言描述需求，AI 先提出計畫與 3D 架構圖，使用者確認後才撰寫程式，經自動檢查與人工審查後部署到 Rancher 管理的叢集。

> 專案暱稱「鐵板麵加雞塊」。名稱的由來：對系統說需求，就像在早餐店跟老闆點餐。

## 狀態

開發初期，尚未發行。第一階段以 Rancher Prime 2.15.2 為基準，只在 ARM64 上實際驗證。

## 目前內容

| 路徑 | 說明 |
|---|---|
| `crates/laoban-audit` | 稽核紀錄：遮蔽後輸出 RFC 5424 syslog 訊息、RFC 5425 封框，以及依大小輪替的本機檔案 |
| `crates/laoban-ai` | AI 呼叫協調層：供應商抽象、錯誤分類與重試、全域 token 預算、送出前遮蔽、提示注入緩解、AI 回應的結構與允許清單驗證 |
| `crates/laoban-mask` | 敏感資料遮蔽引擎：送給 AI 之前與寫入稽核紀錄之前都用同一套規則 |
| `crates/laoban-policy` | 部署審查政策引擎：決定一次變更能不能直接部署、要不要人工審查、由誰審 |
| `web/viewer-prototype` | 3D 架構視圖原型：三層架構、版本比較、細節層級、拖曳調整版面 |
| `crates/laoban-server`、`Containerfile`、`build/` | 後端服務（試吃版 API：`/api/plan`、`/api/apply`、`/api/config`）與容器映像檔建置：SUSE BCI 多階段建置、x86-64／ARM64 原生建置、推送 GHCR（可選 Docker Hub）、cosign v2 簽章與 SBOM，見 [build/README.md](build/README.md) |
| `crates/laoban-m0` | 試吃版流程：AI 計畫與架構圖、產生程式、固定範本的 manifest、commit 到 GitHub |
| `crates/laoban-mockhttp` | 測試用的本機 HTTP 模擬伺服器 |
| `charts/laoban` | 後端服務的 Helm chart（試吃版部署在下游叢集 rke2，不需要 RBAC），見 [README](charts/laoban/README.md) |
| `charts/laoban-crd` | laoban 的 CRD（ProjectPolicy、ChangeRequest、ArchitectureVersion） |
| `extension` | Rancher UI extension（`laoban-ext`）：試吃版頁面，輸入需求、看計畫與 3D 架構圖、確認部署、追蹤 Fleet 狀態。見 [extension/README.md](extension/README.md) |
| `deploy/m0` | 試吃版要事先建立的 namespace 與 Fleet 物件，見 [deploy/m0/README.md](deploy/m0/README.md) |
| `docs/m0-design.md` | 試吃版設計 |

## 開發

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
node --test web/viewer-prototype/*.test.mjs
```

## 支援

laoban 是社群開源專案，不在 SUSE 的支援範圍內。問題請開 GitHub issue。

## 授權

Apache License 2.0，見 [LICENSE](LICENSE)。

---

## English

laoban is a Rancher UI extension that lets users describe what they want in natural language. An AI agent proposes a plan and a 3D architecture view first; code is written only after the user confirms, and deployments go through automated checks and human review before reaching clusters managed by Rancher.

laoban is a community open source project and is not supported by SUSE. Please file issues on GitHub.

Licensed under the Apache License 2.0.
