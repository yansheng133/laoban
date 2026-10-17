# laoban 快速上手

[English](quickstart.en.md) · [日本語](quickstart.ja.md)

照這份做，大約 30 分鐘可以在自己的 Rancher 上用一句話做出第一個應用。laoban 是社群開源專案，不在 SUSE 的支援範圍內；目前是試吃版（0.1.x），還在密集修改。

## 你需要準備

| 項目 | 說明 |
|---|---|
| Rancher 2.15 | Rancher Prime 或社群版。extension 只標示支援 2.15。Rancher server 要能連到 `yansheng133.github.io`（Helm repo）與 `raw.githubusercontent.com`（extension 的檔案） |
| 一個 Rancher 管理的叢集 | 跑 laoban 後端與產生的應用；可以是下游叢集，也可以是 local。架構 x86-64 或 ARM64 都可以（目前只在 ARM64 上完整驗證過） |
| `kubectl`、`helm` 3 | 可以對上面那個叢集操作；第 5、7 節還要能對 Rancher **local** 叢集操作（例如 Rancher 主機上的 `/etc/rancher/rke2/rke2.yaml`），或改用 Rancher 介面的 local 叢集 → **Import YAML** |
| GitHub 帳號 | 一個放產生應用的 repo，以及一支只能寫入那個 repo 的 token |
| Anthropic API key | 或一個相容 Anthropic Messages API 的本機模型端點（例如 Ollama，見 [charts/laoban/README.md](../charts/laoban/README.md) 的「改用本機模型」） |

laoban 每次產生計畫或改程式都會呼叫 AI，費用由你的 Anthropic 帳號支付。後端有每月 token 上限（預設 2,000,000），用完就不再呼叫 AI。

> **注意**：目前這個用量只存在後端記憶體裡，**後端重啟（升級、Pod 重建、節點重開機）就從 0 重新計算**。所以它不能當成花費的保障，**請一定要在 [Anthropic Console](https://console.anthropic.com/) 設定花費上限**。

下面的指令用 `--context downstream` 代表跑 laoban 的叢集、`--context local` 代表 Rancher local 叢集，請換成你自己的 kubeconfig context。

## 1. 在 Rancher 安裝 extension

以下照 Rancher 2.15 的畫面寫。

1. Rancher 左上角選單 → **Extensions**。
2. 右上角 **⋮** → **Manage Repositories** → **Add Repository**：
   - Name：`laoban`
   - Target：**Helm Repository**（HTTP(S) URL pointing to a Helm chart repository index，預設就是這個）
   - Index URL：`https://yansheng133.github.io/laoban`
   - 按 **Create**。
3. 存檔後畫面停在 local 叢集的 Apps → Repositories。從左上角選單再回到 **Extensions** → **Available** 分頁，找到 **laoban** → **Install** → 確認版本（最新的，例如 `0.1.18`）→ **Install**。
4. 安裝完重新整理頁面。**laoban** 出現在左上角選單的 **Global Apps**。

Index URL 打不開時，改用 `https://raw.githubusercontent.com/yansheng133/laoban/gh-pages`，內容一樣。

## 2. 準備 GitHub

1. 建一個新 repo 放產生的應用，例如 `laoban-apps`。**建立時勾選「Add a README file」**，讓它有 `main` 分支。
   - 建議先用公開 repo 試：Fleet 不需要額外的讀取權限。私有 repo 要多建一個 Fleet 讀取用的 Secret，見 [deploy/m0/README.md](../deploy/m0/README.md)。
2. 建一支 **fine-grained personal access token**：
   - Repository access：**Only select repositories**，只選上面那個 repo。
   - Repository permissions：只給 **Contents: Read and write**（Metadata: Read-only 會自動帶上）。
   - 把 token 存成一個檔案，例如 `./github-token`，不要貼在指令裡。
3. Anthropic API key 也存成檔案，例如 `./anthropic-api-key`。

## 3. 在叢集建立 namespace 與 Secret

```bash
# 後端
kubectl --context downstream create namespace laoban-system
# 產生的應用（以 Pod Security restricted 強制）
kubectl --context downstream apply -f https://raw.githubusercontent.com/yansheng133/laoban/v0.1.18/deploy/m0/namespace-laoban-dev.yaml

# 金鑰與 token 從檔案讀，不放進 values，也不會留在 shell history
kubectl --context downstream -n laoban-system create secret generic laoban-anthropic --from-file=api-key=./anthropic-api-key
kubectl --context downstream -n laoban-system create secret generic laoban-github --from-file=token=./github-token
```

## 4. 安裝後端

先查出跑 laoban 的叢集在 Rancher 裡的 ID（`c-m-` 開頭，local 叢集是 `local`）：

```bash
kubectl --context local get clusters.management.cattle.io \
  -o custom-columns=ID:.metadata.name,NAME:.spec.displayName
```

`clusterId` 只用來組「打開應用」的網址（應用經 Rancher 代理開啟）；頁面找後端不需要它。查叢集 ID 需要 local 叢集的權限，也可以在 Rancher 介面打開那個叢集，網址列 `/c/` 後面那一段就是（例如 `c-m-xxxxxxxx`）。

建立 `values-laoban.yaml`（把 `<...>` 換掉）：

```yaml
github:
  repo: <你的 GitHub 帳號>/laoban-apps
  existingSecret: laoban-github
  secretKey: token
anthropic:
  existingSecret: laoban-anthropic
  secretKey: api-key
config:
  # Anthropic 的模型 ID
  model: claude-sonnet-5-5
  # 每月 token 上限
  tokenCap: 2000000
  targetNamespace: laoban-dev
  # 上面查到的叢集 ID
  clusterId: <c-m-xxxxxxxx>
  # 後端訊息的語言：en（預設）或 zh-Hant
  language: en
```

安裝（先裝 CRD，再裝後端）：

```bash
helm repo add laoban https://yansheng133.github.io/laoban
helm repo update
helm --kube-context downstream install laoban-crd laoban/laoban-crd -n laoban-system --version 0.1.18
helm --kube-context downstream install laoban laoban/laoban -n laoban-system -f values-laoban.yaml --version 0.1.18
```

發布的 chart 已經填好這個版本的映像檔 digest（`ghcr.io/yansheng133/laoban-server`），不用另外指定。

確認後端就緒：

```bash
kubectl --context downstream -n laoban-system get pods
# AVAILABLE 要是 True
kubectl --context downstream get apiservice v1alpha1.api.laoban.yansheng133.github.io
```

## 5. 讓 Fleet 部署產生的應用

在 **Rancher local 叢集**建立 GitRepo，Fleet 會把 repo 裡 `apps/` 底下的應用部署到 `laoban-dev`。

先查 Fleet 裡的叢集名稱（不是上面的 `c-m-` ID）：

```bash
kubectl --context local -n fleet-default get clusters.fleet.cattle.io
```

下載兩個範本，把 `<owner>` 換成你的 GitHub 帳號、`clusterName: rke2` 換成上面查到的名稱，再套用：

```bash
curl -fsSLO https://raw.githubusercontent.com/yansheng133/laoban/v0.1.18/deploy/m0/fleet-gitrepo.yaml
curl -fsSLO https://raw.githubusercontent.com/yansheng133/laoban/v0.1.18/deploy/m0/fleet-gitreporestriction.yaml
# 編輯 fleet-gitrepo.yaml 之後：
kubectl --context local apply -f fleet-gitreporestriction.yaml
kubectl --context local apply -f fleet-gitrepo.yaml
```

> **注意**：`fleet-gitreporestriction.yaml` 限制的是整個 `fleet-default` workspace：套用後，那裡的每個 GitRepo 都只能部署到 `laoban-dev`。如果 `fleet-default` 已經有其他 GitRepo（`kubectl --context local -n fleet-default get gitrepos`），**先不要套用這個檔案**，只套用 `fleet-gitrepo.yaml`。說明見 [deploy/m0/README.md](../deploy/m0/README.md)。

## 6. 開始用

介面語言在 Rancher 右上角的使用者選單 → Preferences 切換（繁體中文、English、日本語）。以下用繁體中文介面的字。

1. Rancher 左上角選單 → **Global Apps** → **laoban** → **總覽**。頁面會自己找到後端（第一次大約幾秒）。
2. 在左邊用一句話描述想要的應用，例如「一個簡單的留言板，可以新增和刪除留言」，按 **產生計畫**。
3. 看 AI 的計畫和 3D 架構圖，沒問題就按 **確認並部署**。程式寫進 GitHub 後由 Fleet 部署，就緒後按 **打開應用**。
4. 之後在 **應用** 打開這個應用：可以手動改程式、比較或還原版本，也可以開一輪「意見收斂」，讓團隊在 3D 圖上提意見，由 AI 收斂成一份修改計畫，大家確認後才發布。

頁面左上的 **收合選單** 可以把 Rancher 的側邊選單收起來，留更多空間給工作區。

## 7. 讓其他人使用

- Rancher 管理者馬上就能用。
- 其他人：在 `laoban-dev` 有內建 **edit** 角色的人可以使用，有 **view** 的人可以看。
- 用 Rancher Project 管理的話，在 Rancher **local** 叢集套用 Project 角色（要用能管理 local 叢集的 kubeconfig，例如 Rancher 主機上的 `/etc/rancher/rke2/rke2.yaml`；或在 Rancher 介面的 local 叢集 → **Import YAML** 貼上這個檔案），再把成員設成 **laoban user** 或 **laoban viewer**：

```bash
kubectl --context local apply -f https://raw.githubusercontent.com/yansheng133/laoban/v0.1.18/deploy/rancher/roletemplate.yaml
```

  詳見 [deploy/rancher/README.md](../deploy/rancher/README.md)。

## 疑難排解

| 現象 | 檢查 |
|---|---|
| Extensions 頁的 laoban 卡片有紅色驚嘆號，左上選單沒有 laoban | Rancher server 能不能連到 `raw.githubusercontent.com`；瀏覽器 console 的「Failed to load script」網址；extension 是不是 0.1.18 以後的版本（0.1.17 有這個問題） |
| 頁面寫「找不到 laoban 後端」 | `kubectl --context downstream get apiservice v1alpha1.api.laoban.yansheng133.github.io` 的 AVAILABLE；你是否有 `laoban-dev` 的 edit／view 或 laoban user／viewer 角色 |
| 產生計畫失敗、提到 token 或花費上限 | Anthropic Console 的額度；後端每月 token 上限（`config.tokenCap`） |
| 「寫入 GitHub 失敗」 | token 是否只給了那個 repo 的 Contents: Read and write、repo 是否有 `main` 分支；後端 log：`kubectl --context downstream -n laoban-system logs deploy/laoban` |
| 部署一直沒好 | Rancher → Continuous Delivery → Git Repos → `laoban-apps` 的狀態；Pod 是否被 Pod Security 擋下 |
| 意見收斂顯示「請管理者先升級 laoban-crd chart」 | 沒裝 `laoban-crd`，或版本比後端舊 |
| 升級後第一次 `kubectl get rounds` 報錯，第二次就好 | kubectl 的 discovery 快取還是舊的，不用處理 |

## 移除

```bash
helm --kube-context downstream uninstall laoban -n laoban-system
helm --kube-context downstream uninstall laoban-crd -n laoban-system
kubectl --context local -n fleet-default delete gitrepo laoban-apps
```

extension 在 Rancher → Extensions → Installed → laoban → Uninstall。產生的應用留在 `laoban-dev` 與你的 GitHub repo 裡，不再需要時自行刪除。
