# 試吃版（M0）部署：事先建立一次的物件

試吃版的流程與分工見 `docs/m0-design.md`。laoban 後端**不寫入任何叢集**，產生的應用由 Fleet 從 GitHub 拉下來部署；這個目錄放的是 Fleet 與目標 namespace 這幾個要由管理者事先建立一次的物件。後端本身用 `charts/laoban` 安裝（見 `charts/laoban/README.md` 的「M0 安裝」）。

| 檔案 | 套用到 | 用途 |
|---|---|---|
| `namespace-laoban-dev.yaml` | 下游叢集 `rke2` | 產生的應用所在的 namespace `laoban-dev`，以 Pod Security Admission 強制 `restricted` |
| `fleet-gitreporestriction.yaml` | Rancher local 叢集 | `GitRepoRestriction`：workspace `fleet-default` 裡的 GitRepo 只能部署到 `laoban-dev` |
| `fleet-gitrepo.yaml` | Rancher local 叢集 | `GitRepo` `laoban-apps`：拉 `laoban-apps` repo 的 `apps/`，部署到叢集 `rke2` 的 `laoban-dev` |
| `ci/` | 不部署 | CI 用的測試腳本與測試用 Pod（見「驗證」） |

## 套用順序

```bash
# 1. 下游叢集 rke2：目標 namespace
kubectl --context rke2 apply -f deploy/m0/namespace-laoban-dev.yaml

# 2. 確認 Fleet 裡的叢集名稱（GitRepo 的 clusterName 要填這個，不是 c-m-26kbxk5f）
kubectl --context local -n fleet-default get clusters.fleet.cattle.io

# 3. Rancher local 叢集：先限制、再建 GitRepo（先把 fleet-gitrepo.yaml 的 <owner> 換掉）
kubectl --context local apply -f deploy/m0/fleet-gitreporestriction.yaml
kubectl --context local apply -f deploy/m0/fleet-gitrepo.yaml

# 4. 看 Fleet 狀態
kubectl --context local -n fleet-default get gitrepo laoban-apps
kubectl --context local -n fleet-default get bundles
```

## 各檔說明

### `namespace-laoban-dev.yaml`

- 標籤 `pod-security.kubernetes.io/enforce: restricted`：不符合 restricted 的 Pod 會被 API server 拒絕［1］［2］。另外加 `audit`、`warn` 同一等級，`kubectl apply` Deployment 時會直接印出警告，不會等到 ReplicaSet 建 Pod 時才默默失敗。
- 沒有加 `enforce-version`，政策跟著叢集的 Kubernetes 版本（預設 `latest`）［2］。
- 用 `kubectl` 建立的 namespace 不屬於任何 Rancher 專案；要在 Rancher UI 依專案管理，再把它移進專案。

### `fleet-gitreporestriction.yaml`

- `allowedTargetNamespaces: [laoban-dev]`：設了這個欄位後，同一個 workspace 的每個 GitRepo **都必須**設 `targetNamespace`，而且必須在清單內；叢集層級的資源（ClusterRole、Namespace 等）一律不能部署［3］［4］。
- 欄位直接放在物件最上層，**沒有 `spec`**（Fleet v0.16.2 原始碼［4］；CI 用 strict 驗證確認放進 `spec` 會被拒絕）。
- **注意：GitRepoRestriction 套用到整個 namespace（workspace）**，不只 `laoban-apps`。`fleet-default` 是 Rancher 放所有下游叢集的 workspace［3］，如果裡面已經有其他 GitRepo，它們沒有 `targetNamespace: laoban-dev` 就會被這個限制擋下。套用前先 `kubectl --context local -n fleet-default get gitrepos` 確認；已有其他 GitRepo 時，要改用另一個 workspace（得把 `rke2` 叢集移過去）或改用 `Policy`。
- Fleet 文件把 `GitRepoRestriction` 標為已淘汰，建議改用 `Policy` 資源［3］；Rancher v2.15.2 隨附的 Fleet v0.16.2 已經有 `policies.fleet.cattle.io` CRD。試吃版照規格先用 GitRepoRestriction，第一階段再評估換成 Policy。

### `fleet-gitrepo.yaml`

- `repo`：`https://github.com/<owner>/laoban-apps`，**把 `<owner>` 換成實際的 GitHub 帳號**；`branch: main`、`paths: [apps]`［5］。`apps/` 底下每個有 `fleet.yaml` 的目錄各自成為一個 bundle，名稱是 GitRepo 名稱加路徑（例如 `laoban-apps-apps-breakfast`）［6］。
- `targetNamespace: laoban-dev`：GitRepoRestriction 要求一定要設［3］。所有資源都放進這個 namespace，遇到叢集層級資源會失敗［5］。
- `targets[0].clusterName: rke2`：填的是 `clusters.fleet.cattle.io` 物件的名稱（Fleet 文件：在 Rancher 裡使用時填 clusters.fleet.cattle.io 資源的名稱），不是 Rancher 叢集 ID `c-m-26kbxk5f`［7］。用上面第 2 步確認。
- **repo 是私有的話**：Fleet 需要讀取權限。在 `fleet-default` 建一個 `kubernetes.io/basic-auth` 的 Secret（username 填 GitHub 帳號、password 填只有該 repo「Contents: Read-only」的 fine-grained token），再把 GitRepo 的 `clientSecretName` 打開［5］。這支 token 跟後端寫入用的 token 分開，後端那支有寫入權限，不要給 Fleet。
- Fleet 以輪詢方式偵測新 commit，間隔可用 GitRepo 的 `pollingInterval` 調整［5］；要即時觸發可以之後再設 GitHub webhook。

## 產生的應用要符合 `restricted`

規格（`docs/m0-design.md`「產生的應用」）裡的安全設定逐項對照 restricted［1］：

| restricted 要求 | 規格 | 結果 |
|---|---|---|
| 卷只能是 configMap、csi、downwardAPI、emptyDir、ephemeral、persistentVolumeClaim、projected、secret | ConfigMap 掛程式；唯讀根檔案系統要寫入時再加 emptyDir | 符合 |
| `allowPrivilegeEscalation: false` | 有 | 符合 |
| `runAsNonRoot: true` | 有 | 符合 |
| `runAsUser` 不可為 0 | 65532 | 符合 |
| `seccompProfile.type` 為 `RuntimeDefault` 或 `Localhost` | `RuntimeDefault` | 符合 |
| `capabilities.drop` 含 `ALL`，`add` 只能是 `NET_BIND_SERVICE` | 去掉所有 capabilities | 符合（port 8080 不需要 `NET_BIND_SERVICE`） |

`ci/psa-restricted-pod.yaml` 是照規格寫的應用 Pod，CI 確認它在 `laoban-dev` 會被接受；`ci/psa-baseline-pod.yaml` 少了這些設定，CI 確認它會被拒絕（證明標籤真的有在擋）。後端產生 manifest 時要注意：

- `securityContext` 要寫在每個 container（或 Pod 層級的 `seccompProfile`、`runAsNonRoot` 加上每個 container 的 `allowPrivilegeEscalation`、`capabilities.drop`）。restricted 的 `allowPrivilegeEscalation` 與 `capabilities` 只看 container 層級。
- 根檔案系統唯讀時，Python 寫不了 `__pycache__`（不影響執行），建議設 `PYTHONDONTWRITEBYTECODE=1`；需要暫存檔就掛 emptyDir 到 `/tmp`。
- Pod 被 PSA 拒絕時，Deployment 本身會建立成功，只是 ReplicaSet 建不出 Pod；Fleet 的 Bundle 會停在未就緒。

## 驗證

`ci/test-m0.sh` 對真正的 API server 檢查（CI 的 `chart` job 在 k3s v1.34 上執行）：

1. 安裝 Fleet v0.16.2 的 CRD（Rancher v2.15.2 隨附的版本），以 server-side dry run 加 `--validate=strict` 套用 GitRepo 與 GitRepoRestriction：欄位名稱或位置錯了（例如放進 `spec`、`targets` 用錯欄位）會被拒絕。kubeconform 沒有 Fleet 的 schema，所以這兩個檔案在 kubeconform 那一步略過。
2. 建立 `laoban-dev`，確認 enforce 標籤，再用上面兩個測試 Pod 確認 restricted 有效。

本機執行：

```bash
curl -fsSL -o /tmp/fleet-crds.yaml \
  https://raw.githubusercontent.com/rancher/fleet/v0.16.2/charts/fleet-crd/templates/crds.yaml
KUBECTL="kubectl --context <拋棄式測試叢集>" deploy/m0/ci/test-m0.sh /tmp/fleet-crds.yaml
```

## 來源

1. Pod Security Standards（Restricted 各項控制）— https://kubernetes.io/docs/concepts/security/pod-security-standards/
2. Pod Security Admission（namespace 標籤、`<MODE>-version`）— https://kubernetes.io/docs/concepts/security/pod-security-admission/
3. Fleet「Namespaces」（GitRepoRestriction、`allowedTargetNamespaces` 必須搭配 `targetNamespace`、`fleet-default` workspace、淘汰改用 Policy）— https://fleet.rancher.io/explanations/namespaces
4. Fleet v0.16.2 `GitRepoRestriction` 型別定義 — https://github.com/rancher/fleet/blob/v0.16.2/pkg/apis/fleet.cattle.io/v1alpha1/gitreporestriction_types.go
5. Fleet GitRepo 參考（`repo`、`branch`、`paths`、`targetNamespace`、`clientSecretName`）— https://fleet.rancher.io/reference/ref-gitrepo ；型別定義 https://github.com/rancher/fleet/blob/v0.16.2/pkg/apis/fleet.cattle.io/v1alpha1/gitrepo_types.go
6. Fleet「Git Repository Contents」（每個 fleet.yaml 一個 bundle、bundle 名稱）— https://fleet.rancher.io/explanations/gitrepo-content
7. Fleet「Mapping to Downstream Clusters」（`targets` 的 `clusterName`、`clusterSelector` 等欄位）— https://fleet.rancher.io/how-tos-for-users/gitrepo-targets
8. Rancher v2.15.2 隨附的 Fleet 版本（`fleetVersion: 110.0.2+up0.16.2`）— https://github.com/rancher/rancher/blob/v2.15.2/build.yaml
