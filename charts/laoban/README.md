# laoban backend Helm chart

laoban 後端服務的 Helm chart，部署在 Rancher 的 local 叢集（管理叢集）。開發初期，尚未發行。

## 三個 chart 的分工

| Chart | 位置 | 內容 | 誰產生 |
|---|---|---|---|
| `laoban` | `charts/laoban` | 後端 Deployment、Service、ServiceAccount、RBAC、PVC | 手寫（本目錄） |
| `laoban-crd` | `charts/laoban-crd` | CRD：`ProjectPolicy`、`ChangeRequest`、`ArchitectureVersion` | 手寫 |
| `laoban-ext` | 不在 repo 內 | UI extension 本體 | **由 extension 建置工具產生，不手寫** |

UI extension 的 Helm chart 由 `yarn publish-pkgs`（或官方可重用的 GitHub workflow `build-extension-charts.yml`）依 `pkg/<extension>/package.json` 產生，產出 `assets/`、`charts/`、`extensions/` 與 `index.yaml`［1］。所以 `laoban-ext` 不在這裡手寫，相容性 annotation 寫在 extension 的 `package.json` 的 `rancher.annotations`：

- `catalog.cattle.io/ui-extensions-version`：Rancher 2.10 起（Extensions API v3、Shell 3.x［15］）必須 `>= 3.0.0`，官方建議上限 `< 4.0.0`，也就是 `">= 3.0.0 < 4.0.0"`［2］。
- `catalog.cattle.io/rancher-version`、`catalog.cattle.io/kube-version`：semver 範圍，分別判斷 Rancher 版本與 Kubernetes 版本是否相容［3］。

## 安裝

固定 namespace `laoban-system`、release 名稱 `laoban`（`Chart.yaml` 的 `catalog.cattle.io/namespace`、`catalog.cattle.io/release-name`［4］）。UI extension 用固定的 Service 位址呼叫後端，所以這兩個值不給改。

```bash
# 1. CRD（Rancher Apps 會因為 catalog.cattle.io/auto-install 先自動裝這個）
helm install laoban-crd charts/laoban-crd -n laoban-system --create-namespace

# 2. 事先建好 Secret（金鑰不放 values）
kubectl -n laoban-system create secret generic laoban-anthropic --from-file=api-key=./anthropic-api-key

# 3. 後端
helm install laoban charts/laoban -n laoban-system \
  --set image.repository=<registry>/<namespace>/laoban-server \
  --set image.digest=sha256:<64 hex> \
  --set anthropic.existingSecret=laoban-anthropic
```

### 必填值

| 值 | 說明 |
|---|---|
| `image.repository` | 預設是佔位字串 `docker.io/REPLACE_ME/laoban-server`，沒改就無法 render |
| `image.digest` | 一律用 digest 拉映像檔，格式 `sha256:<64 個小寫十六進位>`；沒填或格式不對 render 失敗。`image.tag` 只當資訊顯示 |
| `anthropic.existingSecret` | 既有 Secret 名稱，key 預設 `api-key`。金鑰永遠不放在 values |

`replicaCount` 只允許 1（PoC 一律單一副本）。稽核暫存區是 ReadWriteOnce 卷，多副本要先改暫存設計，HA 留到之後。更新策略用 `Recreate`，避免新舊 Pod 同時掛同一個 RWO 卷而卡住。

## 存取方式：沒有 Ingress

Service 只有 ClusterIP，不建 Ingress。UI extension 經由 Rancher 的 Kubernetes API proxy 呼叫後端：

```
/k8s/clusters/local/api/v1/namespaces/laoban-system/services/http:laoban:8080/proxy/<path>
```

- 後半段 `/api/v1/namespaces/<ns>/services/[https:]<name>[:<port>]/proxy` 是 Kubernetes API server proxy 的標準格式；預設走 HTTP，要 HTTPS 就在 Service 名稱前加 `https:`［5］。
- 前綴 `/k8s/clusters/<cluster ID>` 把請求送進 Rancher 對該叢集的 Kubernetes API proxy（local 叢集的 ID 是 `local`）。這個前綴在 Rancher 官方文件裡沒找到明文說明，目前的依據是 Rancher 論壇上的說明［6］與 Rancher 官方文件中「monitoring-ui-view 讓使用者經 Rancher proxy 存取 Prometheus／Grafana UI」的描述［7］。**上線前要在 Rancher 2.15.2 實測。**
- 使用者要對這個 Service 的 `services/proxy` 子資源有權限才走得通：HTTP GET 對應 `get`、POST 對應 `create`、PUT 對應 `update`［8］；RBAC 用 `services/proxy` 表示子資源，可以用 `resourceNames` 限定到這個 Service［9］。**本 chart 不授予這個權限**，由管理者依專案需求綁定。

## 身分委派模型

後端自己沒有任何資源權限。它代表使用者呼叫 Kubernetes API 時一律帶 impersonation header（`Impersonate-User`、`Impersonate-Group`、`Impersonate-Extra-*`），API server 依「該使用者」的 RBAC 授權，稽核紀錄同時留下後端與使用者兩個身分［10］。

ServiceAccount 只綁一個 ClusterRole（`laoban-impersonator-<namespace>-<fullname>`），內容只有 `impersonate`：

| apiGroup | resources | 原因 |
|---|---|---|
| `""` | `users`、`groups` | 設定 `Impersonate-User`、`Impersonate-Group` |
| `authentication.k8s.io` | `userextras/principalid`、`userextras/username` | Rancher 自己的 impersonation ServiceAccount（`cattle-impersonation-system`）就是授予這兩個 extra，分別帶 principal ID（例如 `github_user://098`）與使用者名稱［11］；照著做，稽核紀錄才對得上 Rancher |

刻意不給：`serviceaccounts`（不能冒用 ServiceAccount）、`uids`、其他 extra、任何 get／list／create。

在 k3s v1.34.12 實測（ServiceAccount token 直接打 API server）：

- 以 ServiceAccount 本身身分 list pods：拒絕。
- 冒用使用者 `u-test`：通過 impersonation，再依 `u-test` 的權限被拒（符合預期）。
- 帶 `Impersonate-Extra-principalid`、`Impersonate-Extra-username`：允許；帶 `Impersonate-Extra-scopes`：拒絕。
- 冒用 `system:serviceaccount:kube-system:default`：拒絕（沒有 `serviceaccounts` 權限）。
- **冒用群組 `system:masters`：成功。**

### 已知風險

`users`、`groups` 的 `impersonate` 沒有 `resourceNames` 限制，等於可以冒用任何使用者與群組，包括 `system:masters`。使用者與群組無法事先列舉，所以沒辦法用 `resourceNames` 鎖住；Kubernetes 文件也寫明一般的 impersonate 動詞無法限縮到特定動作或資源［10］。實務上這個 ServiceAccount 的 token 等同 local 叢集的 cluster-admin：

- `laoban-system` namespace 只給平台管理者存取（能讀 Secret、能 exec 進 Pod 的人都拿得到 token）。
- 後端程式只能冒用「已通過 Rancher 驗證的當前使用者」，不能接受前端傳來的任意身分。
- Kubernetes 的 ConstrainedImpersonation 功能可以把 impersonation 限縮到特定動作與資源［10］，是之後收斂權限的方向；目前未評估在 Rancher 2.15 支援的 Kubernetes 版本上是否可用。

## 安全設定

- `runAsNonRoot`（UID／GID 65532）、`readOnlyRootFilesystem`、`allowPrivilegeEscalation: false`、`capabilities.drop: [ALL]`、`seccompProfile: RuntimeDefault`。
- 根檔案系統唯讀，所以另外掛 `/tmp`（emptyDir，上限 64Mi）。
- 資源預設：requests 50m／64Mi，limits 500m／256Mi。
- liveness／readiness 都打 `GET /healthz`（port 8080）。**這是假設**：後端（Rust，尚未撰寫）必須在 8080 提供不需驗證的 `/healthz`；路徑可用 `probes.path` 改。

## 持久化：稽核紀錄暫存區

- PVC `<fullname>-spool`，預設 1Gi、ReadWriteOnce，掛在 `/var/spool/laoban`。稽核紀錄先寫這裡，syslog 收到後才清掉。
- `persistence.storageClass` 留空 = 叢集預設 StorageClass（例如 local-path）。叢集沒有預設 StorageClass 時 PVC 會停在 Pending。
- local-path 沒有快照、資料綁在單一節點；syslog 才是稽核紀錄的正本。
- PVC 加了 `helm.sh/resource-policy: keep`，`helm uninstall` 不會刪掉未送出的紀錄。

## 設定

不放任何秘密在 values；只引用既有 Secret／ConfigMap 的名稱。

| 值 | 說明 |
|---|---|
| `anthropic.existingSecret`／`secretKey` | 以 `ANTHROPIC_API_KEY` 環境變數注入 |
| `syslog.enabled`／`host`／`port` | 稽核紀錄送往的 syslog 伺服器，預設 port 6514（syslog over TLS） |
| `syslog.tls.caConfigMap`／`caKey` | 簽發 syslog 伺服器憑證的 CA（ConfigMap） |
| `syslog.tls.clientCertSecret` | mTLS 用戶端憑證，`kubernetes.io/tls` 型別的既有 Secret；伺服器不要求就留空 |

後端讀取的環境變數（`LAOBAN_LISTEN_ADDR`、`LAOBAN_SPOOL_DIR`、`LAOBAN_SYSLOG_*`、`ANTHROPIC_API_KEY`）是 chart 與後端之間的約定，寫後端時要對齊。

## CRD（`laoban-crd` chart）

### 為什麼獨立成一個 chart

Rancher charts 的慣例：CRD chart 是「唯一目的就是在主 chart 之前把 CRD 裝進叢集」的 chart，用來處理 CRD 的自動升級，以及讓使用者能用另一個 release 乾淨地移除 CRD［12］。Helm 文件明寫目前不支援用 Helm 升級或刪除 `crds/` 目錄裡的 CRD，並把「CRD 與使用它的資源分成兩個 chart」列為做法之一［16］。laoban 的 CRD 會隨版本演進，所以獨立成 `laoban-crd`，CRD 放在 `templates/` 讓 `helm upgrade` 能更新。主 chart 用 `catalog.cattle.io/auto-install: laoban-crd=match` 讓 Rancher 先裝相同版本的 CRD chart［4］，render 時也會檢查 API 是否存在（`crdCheck.enabled`）。

CRD 預設加 `helm.sh/resource-policy: keep`（`keepOnUninstall: true`）：刪掉 CRD 會連同所有 ProjectPolicy、ChangeRequest、ArchitectureVersion 一起刪除。

### API group：`laoban.yansheng133.github.io`

CRD 的 group 應該是自己能控制的網域。`laoban.io` 與 `laoban.dev` 這類網域我們沒有，`laoban.github.io` 由 GitHub 帳號 `laoban` 控制（不論該帳號是否存在，都不是本專案）。repo 擁有者是 GitHub 帳號 `yansheng133`，`yansheng133.github.io` 由該帳號控制，所以用它的子網域。版本先用 `v1alpha1`：專案若搬到組織帳號或取得自有網域，要在 v1 之前換 group（換 group 是破壞性變更）。

### 種類

| Kind | 對應 Rust 型別 | 重點 |
|---|---|---|
| `ProjectPolicy` | `laoban_policy::ProjectPolicy` | `developer`／`platformEngineer`：`Exempt`／`NotifyOnly`／`Review`（預設 `Review`）；`productionAlwaysRequiresReview`、`riskBasedReview` 預設 `true`；有 status 子資源 |
| `ChangeRequest` | `ChangeRequest`、`Risk`、`ExposureChange`、`Decision` | spec 建立後不可修改（CEL `self == oldSelf`）；status 由後端寫入：`decision`（`Deploy`／`ReviewRequired`／`Rejected`）、`notify`、`reviewers`、`reasons`、`rejectReason` |
| `ArchitectureVersion` | `web/viewer-prototype/graph-diff.mjs` 的圖模型 | 每個版本一個物件，spec 整個不可修改；`parent` 必須小於 `version`；節點 id 不可重複（list-map）；沒有 status |

- 欄位名稱用 Rust 欄位的 camelCase、列舉值用 Rust variant 的 PascalCase（例如 `database_schema_change` → `databaseSchemaChange`、`notify_only` → `NotifyOnly`）。Rust 端目前以 serde `snake_case` 序列化，直接拿來讀寫 CR 時要另外對應。
- `ArchitectureVersion` 的 `layer`（`network`／`code`／`data`）沿用 viewer 的小寫，讓前端與 API 不必轉換。
- 所有字串、清單都有長度上限，CEL 規則的成本估算才不會超過 API server 的上限［13］。

## 驗證

CI（`.github/workflows/ci.yml` 的 `chart` job）：

1. `helm lint`（兩個 chart，`--kube-version 1.34.0`）。
2. `helm template`，並確認缺 `image.digest`、缺 `anthropic.existingSecret`、沒裝 CRD 時會以明確訊息失敗。
3. `kubeconform -strict -kubernetes-version 1.34.0` 驗證後端 manifest。kubeconform 預設的 schema 集沒有 `CustomResourceDefinition` 本身的 schema，所以 CRD 不走這一步。
4. 啟動 k3s（v1.34，只有 API server、不跑 agent），用 `charts/laoban-crd/ci/test-crds.sh` 套用 CRD 與範例物件：結構化 schema、預設值、列舉、CEL 規則、spec 不可修改、status 規則都由真正的 API server 檢查；後端 manifest 再做一次 server-side dry run。

Rancher Prime 2.15.2 的 local 叢集支援 Kubernetes v1.34～v1.36［14］，所以用最低的 1.34 驗證。

## 來源

1. Publishing an Extension — https://extensions.rancher.io/extensions/next/publishing
2. Rancher 2.10.0 update（`catalog.cattle.io/ui-extensions-version`）— https://extensions.rancher.io/extensions/next/rancher-2.10-support
3. Extension configuration（Configurable Annotations）— https://extensions.rancher.io/extensions/next/extensions-configuration
4. Creating Apps（Chart.yaml annotations）— https://ranchermanager.docs.rancher.com/how-to-guides/new-user-guides/helm-charts-in-rancher/create-apps
5. Access Services Running on Clusters（Manually constructing apiserver proxy URLs）— https://kubernetes.io/docs/tasks/access-application-cluster/access-cluster-services/
6. Rancher 論壇「Exposing services under rancher dns name」（非官方文件）— https://forums.suse.com/t/exposing-services-under-rancher-dns-name/20106
7. RBAC for Monitoring（monitoring-ui-view）— https://ranchermanager.docs.rancher.com/integrations-in-rancher/monitoring-and-alerting/rbac-for-monitoring
8. Authorization（Determine the Request Verb）— https://kubernetes.io/docs/reference/access-authn-authz/authorization/
9. Using RBAC Authorization（subresources、resourceNames）— https://kubernetes.io/docs/reference/access-authn-authz/rbac/
10. User Impersonation — https://kubernetes.io/docs/reference/access-authn-authz/user-impersonation/
11. Communicating with Downstream User Clusters（cattle-impersonation ClusterRole 範例）— https://ranchermanager.docs.rancher.com/reference-guides/rancher-manager-architecture/communicating-with-downstream-user-clusters
12. rancher/charts wiki「Packages」（Additional Charts CRD Options）— https://github.com/rancher/charts/wiki/2%E2%80%90Packages
13. Extend the Kubernetes API with CustomResourceDefinitions（validation rules、transition rules、cost）— https://kubernetes.io/docs/tasks/extend-kubernetes/custom-resources/custom-resource-definitions/
14. Rancher Manager v2.15.2 support matrix — https://www.suse.com/suse-rancher/support-matrix/all-supported-versions/rancher-v2-15-2/
15. Extensions support matrix（Shell 3.0.0 對應 Rancher 2.10.x 以後）— https://extensions.rancher.io/extensions/next/support-matrix
16. Helm「Custom Resource Definitions」best practices — https://helm.sh/docs/chart_best_practices/custom_resource_definitions/
