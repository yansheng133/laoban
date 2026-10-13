# laoban-server 容器映像檔建置與簽章

本文件說明 `laoban-server` 容器映像檔怎麼建置、推送、簽章，以及 repo 擁有者需要先完成的設定。

相關檔案：

| 路徑 | 說明 |
|---|---|
| `Containerfile` | 多階段建置：SUSE BCI Rust 編譯，SUSE BCI Micro 當執行環境 |
| `.dockerignore` | 建置內容只帶 Rust workspace |
| `.github/workflows/image.yml` | 建置、冒煙測試、推送（GHCR，選用 Docker Hub）、合併 manifest list、簽章、SBOM |
| `build/smoke-test.sh` | 冒煙測試：`/healthz`、`/version`、非 root UID、SIGTERM 正常結束 |

## 映像檔內容

- 執行環境：`registry.suse.com/bci/bci-micro`（含 glibc 與 CA 憑證，不含套件管理工具）。Rust 執行檔動態連結 `libgcc_s.so.1`，所以用 SUSE 文件說明的 `zypper --installroot` 做法，從同版本的 `bci-base` 把 `libgcc_s1` 裝進 Micro 的根目錄。
- 建置環境：`registry.suse.com/bci/rust:1.98`。
- 以 UID/GID `65532:65532` 執行（數字 UID，Kubernetes 的 `runAsNonRoot` 可直接判斷）。
- 監聽 `0.0.0.0:8080`（可用環境變數 `LAOBAN_LISTEN_ADDR` 調整），提供 `GET /healthz`（回 `ok`）與 `GET /version`（JSON：`name`、`version`、`git_sha`）。
- 收到 SIGTERM 會停止接受新連線、等進行中的請求結束後以 0 結束，Pod 停止時不必等到 SIGKILL。
- OCI 標籤：`org.opencontainers.image.source`、`licenses`（Apache-2.0）、`revision`（commit SHA）、`version`。
- 只支援 x86-64（`linux/amd64`）與 ARM64（`linux/arm64`），各自在原生 runner（`ubuntu-24.04`、`ubuntu-24.04-arm`）上建置，不使用 QEMU 模擬，也不交叉編譯。
- `Containerfile` 最後一步會在映像檔內執行一次 `laoban-server --version`：若執行環境缺少執行檔需要的函式庫或 glibc 版本，建置會直接失敗，不會等到部署到叢集才發現。

## Pipeline 流程

預設推送目標是 GitHub Container Registry（GHCR）：`ghcr.io/<repo 擁有者，小寫>/laoban-server`，本 repo 就是 `ghcr.io/yansheng133/laoban-server`。用 workflow 自己的 `GITHUB_TOKEN` 推送，不需要另外設定任何 secret。Docker Hub 改成選用：只有設了 `DOCKERHUB_NAMESPACE` 這個 variable 才會多複製一份過去。

**Pull request**（`build` job）：兩個架構各自建置、載入到 runner 本機，跑 `build/smoke-test.sh`，不推送，也不碰任何 registry 認證。

**推送 `v*` tag 或手動執行（workflow_dispatch）**：

1. `push` job（兩個架構平行）：以 push-by-digest 推送到 GHCR（只有 digest、沒有 tag），從 GHCR 拉回這個 digest 跑冒煙測試，再把 digest 交給下一個 job。
2. `merge` job：用 `docker buildx imagetools create` 把兩個 digest 合成一個 multi-arch manifest list 放在 GHCR，並加上 tag（`1.2.3`、`1.2`、`sha-<完整 commit>`；不產生 `latest`）。有設 `DOCKERHUB_NAMESPACE` 時，再用同一個指令把 manifest list 連同相同的 tag 複製到 Docker Hub，並重新查出 Docker Hub 上的 digest。
3. `sign` job（每個 registry 一份，matrix 由 `merge` 決定：只有 GHCR，或 GHCR 加 Docker Hub）：
   1. 用 cosign **v2**（keyless，GitHub OIDC）簽 manifest list，`--recursive` 連各架構的 manifest 一起簽。NeuVector 只認得 cosign v2 的簽章格式，所以 workflow 明確鎖定 cosign v2，並在簽章前檢查版本是 v2。
   2. 用 syft 為每個架構產生 SPDX JSON 格式的 SBOM，以 cosign attestation（`spdxjson`）附加到該架構的 manifest；SBOM 檔上傳成 workflow artifact（`sbom-spdx-ghcr`、`sbom-spdx-dockerhub`）。
   3. 以 `cosign verify`／`cosign verify-attestation` 驗證剛才的簽章與 attestation。
   4. 在 job summary 印出最終的 manifest list digest 與各架構 digest。

權限（最小權限）：workflow 預設 `permissions: {}`；`build` 只有 `contents: read`；`push` 是 `contents: read` 加 `packages: write`；`merge` 只有 `packages: write`；只有 `sign` 有 `id-token: write`（keyless 簽章用），另加 `packages: write`（cosign 把簽章與 attestation 寫進同一個 GHCR 套件）。所有第三方 action 都以完整 commit SHA 鎖定並註明版本。用 `GITHUB_TOKEN` 推送 GHCR 需要 `packages: write`、登入時使用者名稱用 `github.actor`［GitHub 文件 1］。

## 擁有者必須完成的設定

### 1. GHCR：第一次推送後把套件改成公開（只做一次）

M0 讓叢集不必帶認證就能拉映像檔，所以 `laoban-server` 套件要是**公開**的。個人帳號底下的套件第一次發行時預設是私有、只有自己看得到［GitHub 文件 2］；另一份文件則說由 workflow 用 `GITHUB_TOKEN` 建立的套件會沿用 repo 的可見性［GitHub 文件 1］。兩種說法並存，所以第一次推送 `v*` tag 之後一定要到套件頁面確認，顯示 Private 就照下列步驟改：

1. 到 GitHub 個人頁面的 **Packages**，點 `laoban-server`（或直接開 `https://github.com/users/yansheng133/packages/container/package/laoban-server`）。
2. 右側點 **Package settings**。
3. 拉到最下方 **Danger Zone**，點 **Change visibility**，選 **Public**，輸入套件名稱確認。

注意：**公開之後就不能再改回私有**［GitHub 文件 2］。映像檔裡只有 `laoban-server` 執行檔與 SUSE BCI 基底，沒有任何金鑰。

不想公開時，就讓套件保持私有，改在叢集上提供拉取認證：建立只有 `read:packages` 範圍的 classic personal access token（GHCR 只支援 classic token［GitHub 文件 3］），在 `laoban-system` 建 `kubernetes.io/dockerconfigjson` Secret，再於 Helm values 設 `imagePullSecrets`：

```bash
kubectl -n laoban-system create secret docker-registry ghcr-pull \
  --docker-server=ghcr.io --docker-username=<GitHub 帳號> --docker-password=<read:packages token>
# values: imagePullSecrets: [{name: ghcr-pull}]
```

映像檔帶有 `org.opencontainers.image.source` 標籤，指向本 repo，GHCR 會依這個標籤把套件連結到 repo［GitHub 文件 2］。

### 2. 發行

```bash
git tag v0.1.0
git push origin v0.1.0
```

### 3. 選用：同時發行到 Docker Hub

不設定就只發行到 GHCR。要多一份 Docker Hub 副本時：

1. 在 Docker Hub 先手動建立 `<namespace>/laoban-server`，可見性依需求選擇。若沒有先建立，第一次推送時 Docker Hub 會自動建立 repository，可見性依帳號的預設值決定。私有 repository 在叢集上拉取時需要 `imagePullSecrets`，NeuVector 掃描與驗證簽章也需要這個 registry 的認證資訊。
2. Docker Hub → Account settings → Personal access tokens，建立權限為 **Read & Write** 的 token（簽章與 attestation 也會寫入同一個 repository）。
3. GitHub repo → Settings → Secrets and variables → Actions：

| 種類 | 名稱 | 內容 |
|---|---|---|
| Variable | `DOCKERHUB_NAMESPACE` | Docker Hub 使用者或組織名稱；**有設才會推送到 Docker Hub** |
| Secret | `DOCKERHUB_USERNAME` | 擁有上述 token 的 Docker Hub 使用者名稱 |
| Secret | `DOCKERHUB_TOKEN` | 上一步建立的 access token |

設了 `DOCKERHUB_NAMESPACE` 但 secrets 不完整時，`push` job 會在第一步就失敗並列出缺少的項目。PR 的建置不需要這些設定（也不會讀取）。

## Helm values 一律以 digest 參照

tag 可以被覆寫，digest 不行；簽章與 SBOM 也是綁在 digest 上。Helm values 請填 manifest list 的 digest（`sign` job summary 第一段），不要只填 tag：

```yaml
image:
  repository: ghcr.io/yansheng133/laoban-server
  digest: sha256:<manifest list digest>
```

Docker Hub 副本的 digest 由 `merge` job 另外查出，請用 Docker Hub 那份 job summary 的值。

## 驗證簽章

用 cosign v2（GHCR 套件公開時不必登入；私有時先 `docker login ghcr.io`）：

```bash
IMAGE=ghcr.io/yansheng133/laoban-server@sha256:<manifest list digest>

cosign verify "$IMAGE" \
  --certificate-identity-regexp '^https://github\.com/yansheng133/laoban/\.github/workflows/image\.yml@refs/tags/v.*$' \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com
```

- `--certificate-identity-regexp`：keyless 憑證的身分是 `https://github.com/<job_workflow_ref>`，也就是「這個 repo 的 `image.yml`，從某個 `v*` tag 執行」。只接受 tag 發行的映像檔；若也要接受手動執行（從分支），把 `refs/tags/v.*` 改成對應的 ref。
- `--certificate-oidc-issuer`：GitHub Actions 的 OIDC issuer。

SBOM attestation 附在各架構的 manifest 上，先查出各架構 digest 再驗證：

```bash
docker buildx imagetools inspect "$IMAGE"   # 列出 linux/amd64、linux/arm64 的 digest

cosign verify-attestation --type spdxjson \
  ghcr.io/yansheng133/laoban-server@sha256:<架構 digest> \
  --certificate-identity-regexp '^https://github\.com/yansheng133/laoban/\.github/workflows/image\.yml@refs/tags/v.*$' \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com
```

注意：keyless 簽章會寫入公開的 Sigstore 透明度紀錄（Rekor），紀錄裡有映像檔名稱與 digest 和 workflow 身分。

NeuVector 端以 keyless 方式設定 Sigstore 驗證：OIDC issuer 與身分（或身分的 regexp）填上面兩個值，細節依 NeuVector 文件設定。

## 本機建置

```bash
docker buildx build -f Containerfile \
  --build-arg GIT_SHA="$(git rev-parse HEAD)" \
  --load -t laoban-server:dev .
build/smoke-test.sh laoban-server:dev
```

在 x86-64 機器上會產生 `linux/amd64`，在 ARM64 機器上會產生 `linux/arm64`。

## 參考資料

- SUSE BCI Micro／Minimal 內容與用途：<https://documentation.suse.com/container/all/html/Container-guide/index.html>
- 用 zypper `--installroot` 把套件裝進 BCI Micro：<https://opensource.suse.com/bci-docs/guides/deploy-using-zypper>
- 在 BCI Micro 加入使用者：<https://opensource.suse.com/bci-docs/guides/adding-users>
- buildx image exporter 的 `push-by-digest`、`name-canonical` 參數：<https://docs.docker.com/build/exporters/image-registry/>
- Docker 多平台建置與原生 runner：<https://docs.docker.com/build/ci/github-actions/multi-platform/>
- Fulcio：GitHub Actions 憑證身分格式 `https://github.com/{job_workflow_ref}`：<https://github.com/sigstore/fulcio/blob/main/docs/oidc.md>
- GitHub 文件 1：用 GitHub Actions 發行套件（`GITHUB_TOKEN`、`packages: write`、`docker/login-action` 登入 GHCR）：<https://docs.github.com/en/packages/managing-github-packages-using-github-actions-workflows/publishing-and-installing-a-package-with-github-actions>
- GitHub 文件 2：套件的存取控制與可見性（個人帳號套件預設私有、改公開的步驟、公開後不能改回私有、以 `org.opencontainers.image.source` 連結 repo）：<https://docs.github.com/en/packages/learn-github-packages/configuring-a-packages-access-control-and-visibility>
- GitHub 文件 3：Container registry（只支援 classic personal access token、拉取需要 `read:packages`）：<https://docs.github.com/en/packages/working-with-a-github-packages-registry/working-with-the-container-registry>
