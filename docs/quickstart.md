# laoban 快速上手

[English](quickstart.en.md) · [日本語](quickstart.ja.md)

照這份做，大約 30 分鐘可以在自己的 Rancher 上用一句話做出第一個應用。laoban 是社群開源專案，不在 SUSE 的支援範圍內；目前是試吃版（0.1.x），還在密集修改。

## 你需要準備

| 項目 | 說明 |
|---|---|
| Rancher 2.15 | Rancher Prime 或社群版。extension 只標示支援 2.15。Rancher server 要能連到 `yansheng133.github.io`（Helm repo）與 `raw.githubusercontent.com`（extension 的檔案） |
| 一個 Rancher 管理的下游叢集 | 跑 laoban 後端與產生的應用。**不能用 local 叢集**（Fleet 把 local 叢集放在另一個 workspace，laoban 目前只用 `fleet-default`）。架構 x86-64 或 ARM64 都可以（目前只在 ARM64 上完整驗證過） |
| 對外連線 | 下游叢集要能連到 `api.anthropic.com`（或你的本機模型）、`api.github.com`、`ghcr.io`（後端映像檔）、`registry.suse.com`（產生的應用用的 `bci/python` 映像檔）；Fleet 要能讀你的 GitHub repo |
| `kubectl`、`helm` 3 | 可以對上面那個叢集操作；第 5、7 節還要能對 Rancher **local** 叢集操作（例如 Rancher 主機上的 `/etc/rancher/rke2/rke2.yaml`），或改用 Rancher 介面的 local 叢集 → **Import YAML** |
| GitHub 帳號 | 一個放產生應用的 repo，以及一支只能寫入那個 repo 的 token |
| Anthropic API key | 或一個相容 Anthropic Messages API 的本機模型端點（例如 Ollama，見第 4 節的「改用本機模型」） |

laoban 每次產生計畫或改程式都會呼叫 AI，費用由你的 Anthropic 帳號支付。後端有每月 token 上限（預設 2,000,000），用完就不再呼叫 AI。

> **注意**：用量以 UTC 月份計算，存在叢集的 `TokenUsage` 資源裡（`kubectl --context downstream -n laoban-system get tokenusages`），後端重啟不會歸零。這只是 laoban 自己的計數，不是 Anthropic 的帳單，**請一定也要在 [Anthropic Console](https://console.anthropic.com/) 設定花費上限**。

下面的指令用 `--context downstream` 代表跑 laoban 的叢集、`--context local` 代表 Rancher local 叢集，請換成你自己的 kubeconfig context。

## 1. 在 Rancher 安裝 extension

以下照 Rancher 2.15 的畫面寫。

1. Rancher 左上角選單 → **Extensions**。
2. 右上角 **⋮** → **Manage Repositories** → **Add Repository**：
   - Name：`laoban`
   - Target：**Helm Repository**（HTTP(S) URL pointing to a Helm chart repository index，預設就是這個）
   - Index URL：`https://yansheng133.github.io/laoban`
   - 按畫面下方的 **Add Repository**。
3. 存檔後畫面停在 local 叢集的 Apps → Repositories。從左上角選單再回到 **Extensions** → **Available** 分頁，找到 **laoban** 卡片 → 卡片右上角的 **⋮** → **Install** → 確認版本（最新的，例如 `0.1.22`）→ **Install**。對話框會有「untrusted authors」的黃色提醒，因為 laoban 是社群專案，不是 Rancher 認證的 extension。
4. 安裝完，畫面上方出現 **Extensions changed - reload required**，按 **Reload**（或重新整理頁面）。**laoban** 出現在左上角選單的 **Global Apps**。

Index URL 打不開時，改用 `https://raw.githubusercontent.com/yansheng133/laoban/gh-pages`，內容一樣。

## 2. 準備 GitHub

1. 建一個新 repo 放產生的應用，例如 `laoban-apps`（名稱可以自己取，第 4、5 節換成你的）。**建立時勾選「Add a README file」**，讓它有 `main` 分支。
   - 試用時可以用公開 repo：Fleet 不需要額外的讀取權限。但每個應用的 `laoban.json` 與 README 會記下需求內容（已遮蔽）與做修改的人的 Rancher 使用者 ID，**正式使用請用私有 repo**；私有 repo 要多建一個 Fleet 讀取用的 Secret，見 [deploy/m0/README.md](../deploy/m0/README.md)。
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
kubectl --context downstream apply -f https://raw.githubusercontent.com/yansheng133/laoban/v0.1.22/deploy/m0/namespace-laoban-dev.yaml

# 金鑰與 token 從檔案讀，不放進 values，也不會留在 shell history
kubectl --context downstream -n laoban-system create secret generic laoban-anthropic --from-file=api-key=./anthropic-api-key
kubectl --context downstream -n laoban-system create secret generic laoban-github --from-file=token=./github-token
```

## 4. 安裝後端

先查出跑 laoban 的叢集在 Rancher 裡的 ID（`c-m-` 開頭）：

```bash
kubectl --context local get clusters.management.cattle.io \
  -o custom-columns=ID:.metadata.name,NAME:.spec.displayName
```

`clusterId` 必填，在沒有設定 `apps.url` 時用來組「打開應用」的網址（應用經 Rancher 代理開啟）；頁面找後端不需要它。查叢集 ID 需要 local 叢集的權限，也可以在 Rancher 介面打開那個叢集，網址列 `/c/` 後面那一段就是（例如 `c-m-xxxxxxxx`）。

**應用要放在自己的網址（`apps.url`）。** 沒設定時，應用經 Rancher 代理開啟，跟 Rancher 同一個網址：誰打開應用，應用頁面裡的程式就帶著那個人的 Rancher 登入，能用他的身分呼叫 Rancher API（管理者打開時就是管理者權限）。設定 `apps.url`（`{app}` 會換成應用名稱）後，每個應用多一個 Ingress，從自己的網址打開，瀏覽器不會把 Rancher 的登入給它。需要準備：

- 一個指到叢集 ingress controller 的萬用 DNS 紀錄，例如 `*.apps.example.com`。
- 要用 https 的話，在 `laoban-dev` 放一張萬用憑證，設 `apps.tlsSecretName`；ingress class 不是預設的，設 `apps.ingressClassName`。
- 用了自己網址的應用不再經過 Rancher 權限檢查，網路上連得到的人都能打開。要加登入，用 `apps.ingressAnnotations` 掛上 ingress controller 的驗證（例如 Traefik 的 middleware、ingress-nginx 的 `auth-url`）；這些值會寫進放應用的 repo，不要放秘密。
- 後端啟動時會把每個應用的 Ingress 對齊這些設定（一個 commit）：設定前就有的應用也會補上，改了網址會更新，拿掉 `apps.url` 會刪除。
- 每個應用預設有 NetworkPolicy（`apps.networkPolicy`）：API 只接受自己網頁的連線，兩者都只能連自己的 API 和 DNS。

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
# 應用自己的網址（強烈建議設定，見下方說明）
apps:
  url: https://{app}.apps.<你的網域>/
  # 後端訊息的語言：en（預設）或 zh-Hant
  language: en
```

安裝（先裝 CRD，再裝後端）：

```bash
helm repo add laoban https://yansheng133.github.io/laoban
helm repo update
helm --kube-context downstream install laoban-crd laoban/laoban-crd -n laoban-system --version 0.1.22
helm --kube-context downstream install laoban laoban/laoban -n laoban-system -f values-laoban.yaml --version 0.1.22
```

發布的 chart 已經填好這個版本的映像檔 digest（`ghcr.io/yansheng133/laoban-server`），不用另外指定。

### 改用本機模型

不用 Anthropic API、改用相容 Anthropic Messages API 的本機模型（例如 Ollama）時，`values-laoban.yaml` 改成：

```yaml
anthropic:
  existingSecret: ""   # 不需要 Anthropic API key
ai:
  baseUrl: http://<模型伺服器的位址>:11434   # 下游叢集的 Pod 要連得到
  timeoutSeconds: 900                       # 本機模型比較慢
config:
  model: qwen3-coder:30b                    # 伺服器上的模型名稱
```

其他欄位不變，第 3 節的 `laoban-anthropic` Secret 也不用建。`http://` 是明文連線，請只在內部網路使用。

確認後端就緒：

```bash
kubectl --context downstream -n laoban-system get pods
# READY 要是 1/1
kubectl --context downstream get apiservice v1alpha1.api.laoban.yansheng133.github.io
# AVAILABLE 要是 True
```

## 5. 讓 Fleet 部署產生的應用

在 **Rancher local 叢集**建立 GitRepo，Fleet 會把 repo 裡 `apps/` 底下的應用部署到 `laoban-dev`。

先查 Fleet 裡的叢集名稱（不是上面的 `c-m-` ID）：

```bash
kubectl --context local -n fleet-default get clusters.fleet.cattle.io
```

下載兩個範本，在 `fleet-gitrepo.yaml` 裡把 `https://github.com/<owner>/laoban-apps` 換成你的 repo、`clusterName: rke2` 換成上面查到的名稱，再套用。GitRepo 本身的名稱（`metadata.name: laoban-apps`）**不要改**，頁面用這個名稱追蹤部署狀態：

```bash
curl -fsSLO https://raw.githubusercontent.com/yansheng133/laoban/v0.1.22/deploy/m0/fleet-gitrepo.yaml
curl -fsSLO https://raw.githubusercontent.com/yansheng133/laoban/v0.1.22/deploy/m0/fleet-gitreporestriction.yaml
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
- 其他人：在 `laoban-dev` 有內建 **edit** 角色的人可以使用，有 **view** 的人可以看。他們通常讀不到 Fleet 的狀態，頁面會改看 `laoban-dev` 裡的 Deployment 判斷新版本是否已經跑起來，不用另外給權限。
- 用 Rancher Project 管理的話，在 Rancher **local** 叢集套用 Project 角色（要用能管理 local 叢集的 kubeconfig，例如 Rancher 主機上的 `/etc/rancher/rke2/rke2.yaml`；或在 Rancher 介面的 local 叢集 → **Import YAML** 貼上這個檔案），再把成員設成 **laoban user** 或 **laoban viewer**：

```bash
kubectl --context local apply -f https://raw.githubusercontent.com/yansheng133/laoban/v0.1.22/deploy/rancher/roletemplate.yaml
```

  詳見 [deploy/rancher/README.md](../deploy/rancher/README.md)。

## 疑難排解

| 現象 | 檢查 |
|---|---|
| Extensions 頁的 laoban 卡片有紅色驚嘆號，左上選單沒有 laoban | Rancher server 能不能連到 `raw.githubusercontent.com`；瀏覽器 console 的「Failed to load script」網址；extension 是不是 0.1.18 以後的版本（0.1.17 有這個問題） |
| 頁面寫「找不到 laoban 後端」 | `kubectl --context downstream get apiservice v1alpha1.api.laoban.yansheng133.github.io` 的 AVAILABLE；你是否有 `laoban-dev` 的 edit／view 或 laoban user／viewer 角色 |
| 產生計畫失敗、提到 token 或花費上限 | Anthropic Console 的額度；後端每月 token 上限（`config.tokenCap`） |
| 「寫入 GitHub 失敗」 | token 是否只給了那個 repo 的 Contents: Read and write、repo 是否有 `main` 分支；後端 log：`kubectl --context downstream -n laoban-system logs deploy/laoban` |
| 部署一直沒好 | Rancher → Continuous Delivery → Git Repos → `laoban-apps` 的狀態；Pod 是否被 Pod Security 擋下；下游叢集能不能拉 `registry.suse.com/bci/python` |
| 剛建好 GitRepo，狀態沒有任何資源 | 正常：repo 裡還沒有 `apps/`，第一個應用部署後才會有 |
| 產生計畫失敗，提到模型不存在（not found） | `config.model` 的模型 ID 是否正確；本機模型要先在伺服器上下載 |
| 意見收斂顯示「請管理者先升級 laoban-crd chart」 | 沒裝 `laoban-crd`，或版本比後端舊 |
| 後端重啟後 token 用量從 0 開始，或還沒確認的計畫不見了 | `laoban-crd` 比 0.1.19 舊（後端 log 有「not installed」）。升級 `laoban-crd` 後 30 秒內開始保存 |
| 想在月中把 token 用量歸零 | 刪掉當月的紀錄再重啟後端：`kubectl --context downstream -n laoban-system delete tokenusage tokens-YYYY-MM`（UTC 月份），然後 `kubectl --context downstream -n laoban-system rollout restart deploy/laoban` |
| 升級後第一次 `kubectl get rounds` 報錯，第二次就好 | kubectl 的 discovery 快取還是舊的，不用處理 |

## 升級

1. **extension**：Rancher → Extensions → 右上 **⋮** → **Manage Repositories** → `laoban` 那一列的 **⋮** → **Refresh**（Rancher 會快取 repo 清單，不 Refresh 看不到新版）。回到 **Extensions** → **Installed** → laoban 卡片右上的 **⋮** → **Upgrade** → 選新版本 → **Upgrade**，再按畫面上方的 **Reload**。
2. **後端**：**先**升級 CRD，再升級後端（版本換成新版）：

```bash
helm repo update
helm --kube-context downstream upgrade laoban-crd laoban/laoban-crd -n laoban-system --version <新版本>
helm --kube-context downstream upgrade laoban laoban/laoban -n laoban-system -f values-laoban.yaml --version <新版本>
```

進行中的意見收斂、還沒確認的計畫與當月的 token 用量都存在叢集裡，升級後還在。每一版的注意事項見 [Releases](https://github.com/yansheng133/laoban/releases)。

## 移除

```bash
helm --kube-context downstream uninstall laoban -n laoban-system
helm --kube-context downstream uninstall laoban-crd -n laoban-system
kubectl --context local -n fleet-default delete gitrepo laoban-apps
```

extension 在 Rancher → Extensions → **Installed** → laoban 卡片右上角的 **⋮** → **Uninstall**。產生的應用留在 `laoban-dev` 與你的 GitHub repo 裡，不再需要時自行刪除。
