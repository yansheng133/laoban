# laoban backend Helm chart

laoban 後端服務的 Helm chart。開發初期，尚未發行。

試吃版（M0，見 `docs/m0-design.md`）的部署位置是**下游叢集 `rke2`**（Rancher 叢集 ID `c-m-26kbxk5f`）的 namespace `laoban-system`，不是 Rancher local 叢集。M0 的後端以 aggregated API 提供服務（見下面的「Aggregated API」），自己的權限只有兩項：讀 `kube-system` 的 `extension-apiserver-authentication`，以及在 `laoban-system` 讀寫意見收斂的 `Round` 資源（v0.1.15 起，讓一輪在重啟後還在）。產生的應用由 Fleet 部署（`deploy/m0/`），後端不寫入其他任何東西。身分模擬（impersonation）、第一階段的 CRD、稽核紀錄暫存卷都留到第一階段，chart 裡保留對應的範本，預設關閉。

## 三個 chart 的分工

| Chart | 位置 | 內容 | 誰產生 |
|---|---|---|---|
| `laoban` | `charts/laoban` | 後端 Deployment、Service、ServiceAccount；第一階段才開的 RBAC（`impersonation.enabled`）與 PVC（`persistence.enabled`） | 手寫（本目錄） |
| `laoban-crd` | `charts/laoban-crd` | CRD：`Round`（M0 的意見收斂）；`ProjectPolicy`、`ChangeRequest`、`ArchitectureVersion`（第一階段） | 手寫 |
| `laoban-ext` | 不在 repo 內 | UI extension 本體 | **由 extension 建置工具產生，不手寫** |

UI extension 的 Helm chart 由 `yarn publish-pkgs`（或官方可重用的 GitHub workflow `build-extension-charts.yml`）依 `pkg/<extension>/package.json` 產生，產出 `assets/`、`charts/`、`extensions/` 與 `index.yaml`［1］。所以 `laoban-ext` 不在這裡手寫，相容性 annotation 寫在 extension 的 `package.json` 的 `rancher.annotations`：

- `catalog.cattle.io/ui-extensions-version`：Rancher 2.10 起（Extensions API v3、Shell 3.x［15］）必須 `>= 3.0.0`，官方建議上限 `< 4.0.0`，也就是 `">= 3.0.0 < 4.0.0"`［2］。
- `catalog.cattle.io/rancher-version`、`catalog.cattle.io/kube-version`：semver 範圍，分別判斷 Rancher 版本與 Kubernetes 版本是否相容［3］。

## M0 安裝

固定 namespace `laoban-system`、release 名稱 `laoban`（`Chart.yaml` 的 `catalog.cattle.io/namespace`、`catalog.cattle.io/release-name`［4］）。UI extension 用固定的 Service 位址呼叫後端，所以這兩個值不給改。`charts/laoban/ci/m0-values.yaml` 就是 M0 的設定，CI 也用它做 lint、template 與 kubeconform。

```bash
# 以下都對下游叢集 rke2 操作
kubectl --context rke2 create namespace laoban-system
# 選用：後端 Pod 符合 restricted，可以讓 namespace 也強制 restricted
kubectl --context rke2 label namespace laoban-system pod-security.kubernetes.io/enforce=restricted

# 1. 事先建好兩個 Secret（金鑰與 token 都不放 values，也不要留在 shell history）
kubectl --context rke2 -n laoban-system create secret generic laoban-anthropic --from-file=api-key=./anthropic-api-key
kubectl --context rke2 -n laoban-system create secret generic laoban-github --from-file=token=./github-token

# 2. CRD（意見收斂的 Round；升級時也要先升級它）
helm --kube-context rke2 install laoban-crd charts/laoban-crd -n laoban-system

# 3. 後端（digest 從 image workflow 的 job summary 複製，見 build/README.md）
helm --kube-context rke2 install laoban charts/laoban -n laoban-system \
  -f charts/laoban/ci/m0-values.yaml \
  --set image.digest=sha256:<manifest list digest>
```

- **先裝或先升級 `laoban-crd`**（v0.1.15 起）：意見收斂存在 `Round` 資源裡。沒裝 CRD 時後端照常啟動、其他功能都能用，只有意見收斂顯示「請管理者先升級 laoban-crd chart」，裝好後 10 秒內恢復。`crds.required` 預設仍是 `false`，所以 render 不會因為沒裝 CRD 而失敗；`Chart.yaml` 也還沒有 `catalog.cattle.io/auto-install: laoban-crd=match`（兩個 chart 的版本號目前一起升，第一階段再加回這個 annotation）。
- GHCR 套件若還是私有，要先依 `build/README.md` 改成公開，或在 `laoban-system` 建 `kubernetes.io/dockerconfigjson` Secret 並設 `imagePullSecrets`。
- GitHub token：fine-grained personal access token，Repository access 只選 `laoban-apps` 那一個 repo，Repository permissions 只給 **Contents: Read and write**（Metadata: Read-only 會自動帶上）。後端用 Git Data API 建 blob、tree、commit 並更新 ref，這些都屬於 Contents 權限。

### 必填值

| 值 | 說明 |
|---|---|
| `image.repository` | 預設 `ghcr.io/yansheng133/laoban-server`；用 Docker Hub 副本時改成 `docker.io/<namespace>/laoban-server` |
| `image.digest` | 一律用 digest 拉映像檔，格式 `sha256:<64 個小寫十六進位>`；沒填或格式不對 render 失敗。`image.tag` 只當資訊顯示 |
| `anthropic.existingSecret` | 既有 Secret 名稱，key 預設 `api-key`，注入成 `ANTHROPIC_API_KEY`。設了 `ai.baseUrl` 時可以留空 |
| `github.existingSecret` | 既有 Secret 名稱，key 預設 `token`，注入成 `LAOBAN_GITHUB_TOKEN` |
| `github.repo` | `<owner>/<repo>`，例如 `yansheng133/laoban-apps`，注入成 `LAOBAN_GITHUB_REPO` |
| `config.model` | 模型 ID，注入成 `LAOBAN_MODEL`：Anthropic 模型 ID，或本機模型在端點上的名稱。後端不寫死模型 |

金鑰與 token 永遠不放在 values，只填既有 Secret 的名稱。

### 改用本機模型（Ollama）

不用 Anthropic API 時，可以把 AI 呼叫改送到相容 Anthropic Messages API 的端點，例如 Ollama（`/v1/messages`，本機版不檢查 API key）。疊加 `ci/m0-local-model-values.yaml`：

```bash
helm upgrade --install laoban charts/laoban -n laoban-system \
  -f charts/laoban/ci/m0-values.yaml -f charts/laoban/ci/m0-local-model-values.yaml \
  --set image.digest=sha256:<...>
```

- `ai.baseUrl`：只寫 scheme、主機與 port（後端自己加 `/v1/messages`），例如 `http://192.168.128.1:11434`。`http://` 是明文，只用在私有網路。
- `ai.timeoutSeconds`：單次 AI 呼叫上限，30～1800 秒，預設 300；本機模型比較慢，overlay 設 900。
- `anthropic.existingSecret` 可以留空，此時不注入 `ANTHROPIC_API_KEY`，後端也不送 `x-api-key`。
- 遮蔽、token 上限等規則照常套用；token 數由端點回報，Ollama 的數字是近似值。
- 本機模型照「只回 JSON」指示的穩定度比較差，`ai_invalid_output` 會比較常見。後端會去掉回應前後多出的說明文字再解析，但內容驗證跟原本一樣嚴格。

Pod 標籤帶有 chart 版號（`helm.sh/chart`、`app.kubernetes.io/version`），所以只要 chart 版號改了，即使映像檔沒變，`helm upgrade` 也會換新 Pod；後端記憶體裡的計畫與意見收斂（進行中的一輪）會一起清掉。

`replicaCount` 只允許 1：M0 的計畫存在後端記憶體，稽核暫存區開啟時也是 ReadWriteOnce 卷，多副本要先改設計，HA 留到之後。更新策略用 `Recreate`，避免新舊 Pod 同時掛同一個 RWO 卷而卡住。

## 存取方式：aggregated API，沒有 Ingress

Service 只有 ClusterIP，不建 Ingress。後端以 Kubernetes aggregated API 掛在所在叢集的 API server 底下（`aggregatedApi.enabled`，預設開啟），UI extension 經 Rancher 的 `/k8s/clusters/<cluster ID>` 代理呼叫：

```
/k8s/clusters/c-m-26kbxk5f/apis/api.laoban.yansheng133.github.io/v1alpha1/namespaces/<config.targetNamespace>/<path>
```

chart 建立的物件：

| 物件 | 用途 |
|---|---|
| `APIService` `v1alpha1.api.laoban.yansheng133.github.io` | 把這個 API group 交給 Service 的 `https` port（8443）；`caBundle` 是下面那張憑證的 CA |
| Secret `<fullname>-apiserver-tls` | 後端的 TLS 憑證與 CA，由 Helm 在第一次安裝時產生（10 年），升級時沿用（`lookup`；離線 `helm template` 一律重新產生） |
| RoleBinding（`kube-system`） | 讓後端 ServiceAccount 讀 `extension-apiserver-authentication`（內建 Role `extension-apiserver-authentication-reader`） |
| Role 與 RoleBinding `<fullname>-state`（release namespace） | 後端存意見收斂：只能對 `laoban.yansheng133.github.io` 的 `rounds` 做 get、list、create、update、delete，沒有 patch、watch，也碰不到其他資源（`templates/state-rbac.yaml`）。使用者沒有這個資源的權限 |
| ClusterRole `laoban-api-edit`／`laoban-api-view` | 使用者權限，帶 `aggregate-to-edit`／`-admin`／`-view` 標籤（`aggregatedApi.aggregateToDefaultRoles`）：在目標 namespace 有內建 edit／admin 的人可以用，有 view 的人可以看 |

- API server 先驗證使用者並用他的 RBAC 授權，才把請求轉給後端；轉送時用自己的 proxy client 憑證，使用者放在 `X-Remote-User`／`X-Remote-Group`。後端只接受這張憑證（docs/m0-design.md 的「身分」）。
- 因為要讀上面的 ConfigMap，開啟 `aggregatedApi` 時 Pod 會掛 ServiceAccount token。
- Rancher 管理者有全部權限。Rancher Project 成員的角色是 Rancher 的 role template，不是內建 edit；`deploy/rancher/roletemplate.yaml` 提供 Project 角色「laoban user」「laoban viewer」，規則與這兩個 ClusterRole 相同（CI 比對），要套用在 Rancher local 叢集（見 `deploy/rancher/README.md`）。
- 前綴 `/k8s/clusters/<cluster ID>` 把請求送進 Rancher 對該叢集的 Kubernetes API proxy（local 叢集的 ID 是 `local`，M0 用 `rke2` 的 ID `c-m-26kbxk5f`，由 `config.clusterId` 設定）［6］［7］。

## 身分委派模型（第一階段，M0 關閉）

`impersonation.enabled` 預設 `false`：不 render ClusterRole、ClusterRoleBinding，ServiceAccount 與 Pod 都設 `automountServiceAccountToken: false`。以下是第一階段打開後的設計。

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

### 權限要給在哪個叢集：Rancher local 叢集

經 Rancher 的 `/k8s/clusters/<cluster ID>` proxy 送出的 impersonation header，是由 **Rancher 自己**先檢查：Rancher 以 SubjectAccessReview 確認呼叫者能不能 `impersonate` 那個使用者、群組與 extra，而這個 SubjectAccessReview 是送到 **local（管理）叢集**，不是下游叢集（Rancher v2.15.2 `pkg/auth/requests/impersonate.go`、`pkg/auth/requests/sar/sar.go`［17］；我們的實測結果相同）。所以第一階段若後端經 Rancher proxy 冒用使用者，`impersonate` 權限要授予在 local 叢集、給後端向 Rancher 驗證時所用的身分；本 chart 的 ClusterRole 只對「直接打本叢集 API server」的呼叫有效。

### 已知風險

目前的 ClusterRole 對 `users`、`groups` 的 `impersonate` 沒有加 `resourceNames`，等於可以冒用任何使用者與群組，包括 `system:masters`。

- Kubernetes RBAC **可以**用 `resourceNames` 把 impersonation 限定到指定的使用者、群組、extra 值與 UID（例如只能冒用 `jane.doe@example.com`、只能帶群組 `developers`）［10］。Rancher 的檢查也帶了被冒用的名稱（SubjectAccessReview 的 `resourceAttributes.name`），所以在 local 叢集授權時同樣可以用 `resourceNames` 限定［17］。本 chart 還沒這樣做，是因為 laoban 的使用者名單還沒固定；第一階段要依實際使用者（Rancher 的 `u-xxxxx` 名稱）收斂。
- 另外，`impersonate` 動詞本身無法限縮到「只能做哪些動作、碰哪些資源」；Kubernetes 的 ConstrainedImpersonation 功能（feature gate）可以做到這一點［10］，目前未評估在 Rancher 2.15 支援的 Kubernetes 版本上是否可用。

在收斂之前，這個 ServiceAccount 的 token 等同所在叢集的 cluster-admin：

- `laoban-system` namespace 只給平台管理者存取（能讀 Secret、能 exec 進 Pod 的人都拿得到 token）。
- 後端程式只能冒用「已通過 Rancher 驗證的當前使用者」，不能接受前端傳來的任意身分。

## 安全設定

- `runAsNonRoot`（UID／GID 65532）、`readOnlyRootFilesystem`、`allowPrivilegeEscalation: false`、`capabilities.drop: [ALL]`、`seccompProfile: RuntimeDefault`。這組設定符合 Pod Security Standards 的 `restricted`，`laoban-system` 可以強制 restricted。
- M0 不掛 ServiceAccount token（`automountServiceAccountToken: false`）。
- 根檔案系統唯讀，所以另外掛 `/tmp`（emptyDir，上限 64Mi）。
- 資源預設：requests 50m／64Mi，limits 500m／256Mi。
- liveness／readiness 都打 `GET /healthz`（port 8080，後端已提供）；路徑可用 `probes.path` 改。

## 持久化：稽核紀錄暫存區

M0 預設 `persistence.enabled: false`：M0 不外送稽核紀錄（只寫後端自己的 log），暫存區用 emptyDir，Pod 刪除就消失。第一階段打開後：

- PVC `<fullname>-spool`，預設 1Gi、ReadWriteOnce，掛在 `/var/spool/laoban`。稽核紀錄先寫這裡，syslog 收到後才清掉。
- 打開時 `persistence.storageClass` 必填，預設 `local-path`。不依賴叢集預設 StorageClass：`rke2` 沒有預設值，PVC 會一直停在 Pending。需要快照時改用 `longhorn`。
- local-path 沒有快照、資料綁在單一節點；syslog 才是稽核紀錄的正本。
- PVC 加了 `helm.sh/resource-policy: keep`，`helm uninstall` 不會刪掉未送出的紀錄。

## 設定

不放任何秘密在 values；只引用既有 Secret／ConfigMap 的名稱。

| 值 | 環境變數 | 說明 |
|---|---|---|
| `containerPort` | `LAOBAN_LISTEN_ADDR` | 預設 `0.0.0.0:8080` |
| `anthropic.existingSecret`／`secretKey` | `ANTHROPIC_API_KEY` | 從 Secret 注入；設了 `ai.baseUrl` 時可省略 |
| `ai.baseUrl` | `LAOBAN_AI_BASE_URL` | 空白＝Anthropic API；否則為相容端點（例如 Ollama） |
| `ai.timeoutSeconds` | `LAOBAN_AI_TIMEOUT_SECONDS` | 單次 AI 呼叫上限，預設 300 |
| `config.model` | `LAOBAN_MODEL` | 必填，模型 ID |
| `config.tokenCap` | `LAOBAN_TOKEN_CAP` | 每月 token 上限，預設 2000000 |
| `github.repo` | `LAOBAN_GITHUB_REPO` | 必填，`<owner>/<repo>` |
| `github.existingSecret`／`secretKey` | `LAOBAN_GITHUB_TOKEN` | 必填，從 Secret 注入 |
| `config.targetNamespace` | `LAOBAN_TARGET_NAMESPACE` | 預設 `laoban-dev` |
| `config.clusterId` | `LAOBAN_CLUSTER_ID` | 預設 `c-m-26kbxk5f`，用來組應用網址 |
| `persistence.mountPath` | `LAOBAN_SPOOL_DIR` | 稽核暫存區路徑 |
| `syslog.enabled`／`host`／`port` | `LAOBAN_SYSLOG_*` | 稽核紀錄送往的 syslog 伺服器，預設 port 6514（syslog over TLS）；M0 關閉 |
| `syslog.tls.caConfigMap`／`caKey` | `LAOBAN_SYSLOG_CA_FILE` | 簽發 syslog 伺服器憑證的 CA（ConfigMap） |
| `syslog.tls.clientCertSecret` | `LAOBAN_SYSLOG_CLIENT_*` | mTLS 用戶端憑證，`kubernetes.io/tls` 型別的既有 Secret；伺服器不要求就留空 |
| `rounds.limits.opinionChars`／`replyChars`／`roundChars`／`opinions`／`replies` | `LAOBAN_ROUND_MAX_*` | 意見收斂的大小限制，空白＝內建值（1000、300、8000、30、20）；只能調低，超過內建值或不是 1 以上的整數時 render 失敗 |
| `crds.required` | — | `true` 時沒裝 `laoban-crd` 就無法 render；M0 預設 `false` |
| `impersonation.enabled` | — | 第一階段的身分模擬 RBAC；M0 預設 `false` |

環境變數是 chart 與後端之間的約定（`docs/m0-design.md`「設定」），寫後端時要對齊。

## CRD（`laoban-crd` chart）

### 為什麼獨立成一個 chart

Rancher charts 的慣例：CRD chart 是「唯一目的就是在主 chart 之前把 CRD 裝進叢集」的 chart，用來處理 CRD 的自動升級，以及讓使用者能用另一個 release 乾淨地移除 CRD［12］。Helm 文件明寫目前不支援用 Helm 升級或刪除 `crds/` 目錄裡的 CRD，並把「CRD 與使用它的資源分成兩個 chart」列為做法之一［16］。laoban 的 CRD 會隨版本演進，所以獨立成 `laoban-crd`，CRD 放在 `templates/` 讓 `helm upgrade` 能更新。M0 只用 `Round`（意見收斂，v0.1.15 起），其他三個是第一階段的：主 chart 預設 `crds.required: false`，`Chart.yaml` 也暫時拿掉 `catalog.cattle.io/auto-install: laoban-crd=match`。第一階段用到 CRD 時，把這個 annotation 加回來讓 Rancher 先裝相同版本的 CRD chart［4］，並設 `crds.required: true`，render 時就會檢查 API 是否存在。

CRD 預設加 `helm.sh/resource-policy: keep`（`keepOnUninstall: true`）：刪掉 CRD 會連同所有 ProjectPolicy、ChangeRequest、ArchitectureVersion 與進行中的意見收斂一起刪除。

`Round` 只由後端寫入：`spec.round` 是後端自己的資料（`crates/laoban-m0/src/rounds.rs`），以 `spec.schema` 標示格式版本，schema 只檢查外層欄位（`app`、`state`、`deadline`）。結束的一輪歸檔到應用 repo 的 `laoban-rounds` 分支後，在下一輪開始時刪除。

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

1. `helm lint`：`laoban` 用兩組 values 各跑一次——`ci/ci-values.yaml`（把 M0 關掉的功能全部打開：impersonation RBAC、CRD 檢查、PVC、syslog mTLS，讓這些範本持續被檢查）與 `ci/m0-values.yaml`（M0 實際安裝的設定）；再加上 `laoban-crd`。全部 `--kube-version 1.34.0`。
2. `helm template` 兩組 values，並確認缺必填值時會以明確訊息失敗：`image.digest`（兩組都測）、`anthropic.existingSecret`、`github.existingSecret`、`github.repo`、`config.model`、`ai.baseUrl` 格式、`ai.timeoutSeconds` 範圍；`crds.required` 為 `true` 而沒裝 CRD（`ci-values.yaml`，以及 M0 打開 `crds.required` 時）；`persistence.enabled` 為 `true` 而 `storageClass` 空白（兩組都測）。
3. 確認 M0 的 render 沒有 ClusterRoleBinding、PVC、`impersonate`；只有一個 Role（`laoban-system` 裡的 `rounds`，沒有 patch、watch 或萬用字元）與兩個 RoleBinding（`kube-system` 讀 ConfigMap、`laoban-state`）；`rounds.limits` 沒設時不傳 `LAOBAN_ROUND_MAX_*`，設了就傳那一個；`rounds.limits` 調高、設 0、非整數或文字時 render 失敗；M0 的環境變數都在、`LAOBAN_TOKEN_CAP` 是 `"2000000"`（不是 `2e+06`）；同時確認 `ci-values.yaml` 的 render 仍有 ClusterRole、ClusterRoleBinding、PVC。
4. `kubeconform -strict -kubernetes-version 1.34.0` 驗證兩份後端 manifest 與 `deploy/m0`。kubeconform 預設的 schema 集沒有 `CustomResourceDefinition` 本身與 Fleet 物件的 schema，這些改由下一步的 API server 檢查。
5. 啟動 k3s（v1.34，只有 API server、不跑 agent）：`charts/laoban-crd/ci/test-crds.sh` 套用 CRD 與範例物件（結構化 schema、預設值、列舉、CEL 規則、spec 不可修改、status 規則）；兩份後端 manifest 做 server-side dry run；`deploy/m0/ci/test-m0.sh` 檢查 Fleet 物件與 `laoban-dev` 的 restricted（見 `deploy/m0/README.md`）。

Rancher Prime 2.15.2 支援 Kubernetes v1.34～v1.36［14］，所以用最低的 1.34 驗證。`Chart.yaml` 的 `kubeVersion: ">= 1.34.0-0"` 現在套用在 `rke2` 叢集上：`rke2` 低於 1.34 時 `helm install` 會拒絕。

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
17. Rancher v2.15.2 impersonation 檢查程式（以 SubjectAccessReview 檢查，`resourceAttributes.name` 帶被冒用的名稱）— https://github.com/rancher/rancher/blob/v2.15.2/pkg/auth/requests/impersonate.go 、https://github.com/rancher/rancher/blob/v2.15.2/pkg/auth/requests/sar/sar.go
