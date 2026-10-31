# laoban クイックスタート

[繁體中文](quickstart.md) · [English](quickstart.en.md) · 日本語

このガイドに沿って進めると、約 30 分で自分の Rancher 上に、一文から最初のアプリを作れます。laoban はコミュニティのオープンソースプロジェクトであり、SUSE のサポート対象外です。現在は試食版（0.1.x）で、頻繁に変更しています。

## 準備するもの

| 項目 | 説明 |
|---|---|
| Rancher 2.15 | Rancher Prime またはコミュニティ版。extension がサポート対象として示しているのは 2.15 のみです。Rancher server から `yansheng133.github.io`（Helm repo）と `raw.githubusercontent.com`（extension のファイル）に接続できる必要があります |
| Rancher が管理する downstream クラスター 1 つ | laoban バックエンドと生成されたアプリを実行します。**local クラスターは使えません**（Fleet は local クラスターを別の workspace に置き、laoban は現在 `fleet-default` だけを使います）。アーキテクチャは x86-64 と ARM64 のどちらでも使えます（現時点で完全に検証済みなのは ARM64 のみ） |
| 外部への接続 | downstream クラスターから `api.anthropic.com`（またはローカルモデル）、`api.github.com`、`ghcr.io`（バックエンドのイメージ）、`registry.suse.com`（生成されたアプリが使う `bci/python` イメージ）に接続できること。Fleet が GitHub repo を読めること |
| `kubectl`、`helm` 3 | 上記のクラスターを操作できること。第 5 節と第 7 節では Rancher の **local** クラスターも操作できる必要があります（例：Rancher ホスト上の `/etc/rancher/rke2/rke2.yaml`）。または Rancher UI の local クラスター → **Import YAML** を使います |
| GitHub アカウント | 生成されたアプリを置く repo と、その repo にのみ書き込める token |
| Anthropic API key | または Anthropic Messages API 互換のローカルモデルのエンドポイント（例：Ollama。第 4 節の「ローカルモデルを使う」を参照） |

laoban は計画を作成したりコードを変更したりするたびに AI を呼び出し、その費用はお使いの Anthropic アカウントに請求されます。バックエンドには月間 token 上限（デフォルト 2,000,000）があり、使い切ると AI を呼び出さなくなります。

> **注意**：使用量は UTC の月単位で数え、クラスターの `TokenUsage` リソースに保存されます（`kubectl --context downstream -n laoban-system get tokenusages`）。バックエンドが再起動しても 0 には戻りません。ただしこれは laoban 自身の集計で、Anthropic の請求額ではありません。**必ず [Anthropic Console](https://console.anthropic.com/) でも支出上限を設定してください**。

以下のコマンドでは、`--context downstream` が laoban を実行するクラスター、`--context local` が Rancher の local クラスターを表します。ご自身の kubeconfig context に置き換えてください。

## 1. Rancher に extension をインストールする

以下は Rancher 2.15 の画面に沿って説明します。

1. Rancher 左上のメニュー → **Extensions**。
2. 右上の **⋮** → **Manage Repositories** → **Add Repository**：
   - Name：`laoban`
   - Target：**Helm Repository**（HTTP(S) URL pointing to a Helm chart repository index。デフォルトでこれが選ばれています）
   - Index URL：`https://yansheng133.github.io/laoban`
   - 画面下の **Add Repository** をクリックします。
3. 保存すると、画面は local クラスターの Apps → Repositories に移ります。左上のメニューから **Extensions** → **Available** タブに戻り、**laoban** のカードを見つけて → カード右上の **⋮** → **Install** → バージョン（最新。例：`0.1.21`）を確認 → **Install**。ダイアログに「untrusted authors」の黄色い注意が出ますが、laoban は Rancher 認定ではないコミュニティのプロジェクトだからです。
4. インストール後、画面上部に **Extensions changed - reload required** が表示されたら **Reload** をクリックします（またはページを再読み込みします）。**laoban** は左上のメニューの **Global Apps** に表示されます。

Index URL が開けない場合は、代わりに `https://raw.githubusercontent.com/yansheng133/laoban/gh-pages` を使ってください。内容は同じです。

## 2. GitHub を準備する

1. 生成されたアプリを置く新しい repo を作成します（例：`laoban-apps`。名前は自由です。第 4、5 節ではご自身の名前に置き換えてください）。`main` ブランチができるように、**作成時に「Add a README file」にチェックを入れてください**。
   - お試しなら公開 repo でも構いません。Fleet に追加の読み取り権限が不要です。ただし各アプリの `laoban.json` と README には要望の内容（マスク済み）と変更した人の Rancher ユーザー ID が記録されるため、**本格的に使う場合はプライベート repo を使ってください**。プライベート repo の場合は、Fleet の読み取り用 Secret を別途作成する必要があります。[deploy/m0/README.md](../deploy/m0/README.md) を参照してください。
2. **fine-grained personal access token** を作成します。
   - Repository access：**Only select repositories** で、上記の repo だけを選択します。
   - Repository permissions：**Contents: Read and write** のみを付与します（Metadata: Read-only は自動的に付与されます）。
   - token はファイル（例：`./github-token`）に保存し、コマンドに貼り付けないでください。
3. Anthropic API key もファイル（例：`./anthropic-api-key`）に保存します。

## 3. クラスターに namespace と Secret を作成する

```bash
# バックエンド
kubectl --context downstream create namespace laoban-system
# 生成されたアプリ（Pod Security restricted で強制）
kubectl --context downstream apply -f https://raw.githubusercontent.com/yansheng133/laoban/v0.1.21/deploy/m0/namespace-laoban-dev.yaml

# key と token はファイルから読み込み、values に含めず、shell history にも残さない
kubectl --context downstream -n laoban-system create secret generic laoban-anthropic --from-file=api-key=./anthropic-api-key
kubectl --context downstream -n laoban-system create secret generic laoban-github --from-file=token=./github-token
```

## 4. バックエンドをインストールする

まず、laoban を実行するクラスターの Rancher 上の ID を調べます（`c-m-` で始まります）。

```bash
kubectl --context local get clusters.management.cattle.io \
  -o custom-columns=ID:.metadata.name,NAME:.spec.displayName
```

`clusterId` は必須で、`apps.url` を設定しないときに「アプリを開く」の URL を組み立てるために使います（アプリは Rancher のプロキシ経由で開きます）。ページがバックエンドを見つけるのには不要です。クラスター ID を調べるには local クラスターの権限が必要です。Rancher UI でそのクラスターを開き、アドレスバーの `/c/` の後ろの部分を見ても確認できます（例：`c-m-xxxxxxxx`）。

**アプリには専用のアドレス（`apps.url`）を設定してください。** 設定しないと、アプリは Rancher のプロキシ経由で Rancher と同じアドレスで開かれます。アプリを開いた人の Rancher ログインのままアプリのページが動き、その人として Rancher API を呼び出せます（管理者が開けば管理者の権限です）。`apps.url` を設定すると（`{app}` はアプリ名に置き換わります）、アプリごとに Ingress が作られ、専用のアドレスで開かれるため、ブラウザーは Rancher のログインをアプリに渡しません。必要なもの：

- クラスターの ingress controller を指すワイルドカード DNS レコード（例：`*.apps.example.com`）。
- https を使う場合は `laoban-dev` にワイルドカード証明書を置き、`apps.tlsSecretName` に設定します。ingress class が既定でなければ `apps.ingressClassName` を設定します。
- 専用アドレスのアプリは Rancher の権限チェックを通らなくなり、アドレスに届く人なら誰でも開けます。ログインを必須にするには、`apps.ingressAnnotations` で ingress controller の認証を付けます（例：Traefik の middleware、ingress-nginx の `auth-url`）。値はアプリのリポジトリに書き込まれるため、秘密は入れないでください。
- バックエンドは起動時に、各アプリの Ingress をこの設定に合わせます（1 つの commit）。設定前からあるアプリにも追加され、アドレスを変えると更新され、`apps.url` を外すと削除されます。
- 各アプリには既定で NetworkPolicy が付きます（`apps.networkPolicy`）。API は自分の Web Pod からの接続だけを受け付け、どちらも自分の API と DNS にしか接続できません。

`values-laoban.yaml` を作成します（`<...>` を置き換えてください）。

```yaml
github:
  repo: <あなたの GitHub アカウント>/laoban-apps
  existingSecret: laoban-github
  secretKey: token
anthropic:
  existingSecret: laoban-anthropic
  secretKey: api-key
config:
  # Anthropic のモデル ID
  model: claude-sonnet-5-5
  # 月間 token 上限
  tokenCap: 2000000
  targetNamespace: laoban-dev
  # 上で調べたクラスター ID
  clusterId: <c-m-xxxxxxxx>
# アプリ専用のアドレス（強く推奨。下の説明を参照）
apps:
  url: https://{app}.apps.<あなたのドメイン>/
  # バックエンドのメッセージの言語：en（デフォルト）または zh-Hant
  language: en
```

インストールします（先に CRD、次にバックエンド）。

```bash
helm repo add laoban https://yansheng133.github.io/laoban
helm repo update
helm --kube-context downstream install laoban-crd laoban/laoban-crd -n laoban-system --version 0.1.21
helm --kube-context downstream install laoban laoban/laoban -n laoban-system -f values-laoban.yaml --version 0.1.21
```

公開されている chart には、このバージョンのイメージ digest（`ghcr.io/yansheng133/laoban-server`）がすでに設定されているため、別途指定する必要はありません。

### ローカルモデルを使う

Anthropic API の代わりに、Anthropic Messages API 互換のローカルモデル（例：Ollama）を使う場合は、`values-laoban.yaml` を次のように変更します。

```yaml
anthropic:
  existingSecret: ""   # Anthropic API key は不要
ai:
  baseUrl: http://<モデルサーバーのアドレス>:11434   # downstream クラスターの Pod から接続できること
  timeoutSeconds: 900                                # ローカルモデルは遅いため
config:
  model: qwen3-coder:30b                             # サーバー上のモデル名
```

その他の項目はそのままです。第 3 節の `laoban-anthropic` Secret も不要です。`http://` は暗号化されないため、内部ネットワークでのみ使ってください。

バックエンドの準備ができたことを確認します。

```bash
kubectl --context downstream -n laoban-system get pods
# AVAILABLE が True であること
kubectl --context downstream get apiservice v1alpha1.api.laoban.yansheng133.github.io
```

## 5. 生成されたアプリを Fleet でデプロイする

**Rancher の local クラスター**に GitRepo を作成します。Fleet が repo 内の `apps/` 配下のアプリを `laoban-dev` にデプロイします。

まず、Fleet 上のクラスター名を調べます（上記の `c-m-` ID ではありません）。

```bash
kubectl --context local -n fleet-default get clusters.fleet.cattle.io
```

2 つのテンプレートをダウンロードし、`fleet-gitrepo.yaml` の `https://github.com/<owner>/laoban-apps` をご自身の repo に、`clusterName: rke2` を上で調べた名前に置き換えてから適用します。GitRepo 自体の名前（`metadata.name: laoban-apps`）は**変更しないでください**。ページはこの名前でデプロイ状況を追跡します。

```bash
curl -fsSLO https://raw.githubusercontent.com/yansheng133/laoban/v0.1.21/deploy/m0/fleet-gitrepo.yaml
curl -fsSLO https://raw.githubusercontent.com/yansheng133/laoban/v0.1.21/deploy/m0/fleet-gitreporestriction.yaml
# fleet-gitrepo.yaml を編集した後：
kubectl --context local apply -f fleet-gitreporestriction.yaml
kubectl --context local apply -f fleet-gitrepo.yaml
```

> **注意**：`fleet-gitreporestriction.yaml` が制限するのは `fleet-default` workspace 全体です。適用すると、そこにあるすべての GitRepo が `laoban-dev` にしかデプロイできなくなります。`fleet-default` にすでに他の GitRepo がある場合（`kubectl --context local -n fleet-default get gitrepos`）は、**このファイルはまだ適用せず**、`fleet-gitrepo.yaml` だけを適用してください。詳しくは [deploy/m0/README.md](../deploy/m0/README.md) を参照してください。

## 6. 使ってみる

UI の言語は、Rancher 右上のユーザーメニュー → Preferences で切り替えます（繁體中文、English、日本語）。以下の文言は日本語 UI のものです。

1. Rancher 左上のメニュー → **Global Apps** → **laoban** → **概要**。ページが自動的にバックエンドを見つけます（初回は数秒かかります）。
2. 左側に作りたいアプリを一文で入力し（例：「メッセージを追加・削除できるシンプルな掲示板」）、**計画を作成** をクリックします。
3. AI の計画と 3D アーキテクチャ図を確認し、問題なければ **確認してデプロイ** をクリックします。コードが GitHub に書き込まれた後、Fleet がデプロイします。準備ができたら **アプリを開く** をクリックします。
4. その後は **アプリ** からこのアプリを開きます。コードを手動で修正したり、バージョンを比較・復元したりできるほか、意見のラウンドを開始して、チームで 3D 図上に意見を出すこともできます。AI が意見を 1 つの変更計画にまとめ、全員が確認してからリリースされます。

ページ左上の **メニューを隠す** で Rancher のサイドメニューを折りたたみ、作業領域を広く使えます。

## 7. 他のユーザーが使えるようにする

- Rancher の管理者はすぐに使えます。
- その他のユーザー：`laoban-dev` で組み込みの **edit** ロールを持つユーザーは利用でき、**view** を持つユーザーは閲覧できます。こうしたユーザーは通常 Fleet の状態を読めませんが、ページは `laoban-dev` の Deployment を見て新しいバージョンが動いたかを判断するため、追加の権限は不要です。
- Rancher Project で管理する場合は、Rancher の **local** クラスターに Project ロールを適用し（local クラスターを管理できる kubeconfig を使います。例：Rancher ホスト上の `/etc/rancher/rke2/rke2.yaml`。または Rancher UI の local クラスター → **Import YAML** にこのファイルを貼り付けます）、メンバーに **laoban user** または **laoban viewer** を割り当てます。

```bash
kubectl --context local apply -f https://raw.githubusercontent.com/yansheng133/laoban/v0.1.21/deploy/rancher/roletemplate.yaml
```

  詳しくは [deploy/rancher/README.md](../deploy/rancher/README.md) を参照してください。

## トラブルシューティング

| 症状 | 確認すること |
|---|---|
| Extensions ページの laoban カードに赤い感嘆符が付き、左上のメニューに laoban がない | Rancher server から `raw.githubusercontent.com` に接続できるか。ブラウザーの console に出る「Failed to load script」の URL。extension が 0.1.18 以降か（0.1.17 にはこの問題があります） |
| ページに「laoban バックエンドが見つかりません」と表示される | `kubectl --context downstream get apiservice v1alpha1.api.laoban.yansheng133.github.io` の AVAILABLE。`laoban-dev` の edit／view、または laoban user／viewer ロールを持っているか |
| 計画の作成に失敗し、token や支出上限について表示される | Anthropic Console の利用枠。バックエンドの月間 token 上限（`config.tokenCap`） |
| GitHub への書き込みに失敗する | token にその repo の Contents: Read and write だけが付与されているか、repo に `main` ブランチがあるか。バックエンドのログ：`kubectl --context downstream -n laoban-system logs deploy/laoban` |
| デプロイがいつまでも完了しない | Rancher → Continuous Delivery → Git Repos → `laoban-apps` のステータス。Pod が Pod Security でブロックされていないか。downstream クラスターが `registry.suse.com/bci/python` を pull できるか |
| 作成したばかりの GitRepo にリソースが何もない | 正常です。repo にまだ `apps/` がないためで、最初のアプリをデプロイすると現れます |
| 計画の生成がモデルの「not found」エラーで失敗する | `config.model` のモデル ID が正しいか。ローカルモデルは先にサーバーにダウンロードしておく |
| 意見のラウンドに、管理者に laoban-crd chart のアップグレードを求めるメッセージが表示される | `laoban-crd` がインストールされていないか、バージョンがバックエンドより古い |
| バックエンドの再起動後に token 使用量が 0 から始まる、または未確認の計画が消える | `laoban-crd` が 0.1.19 より古い（バックエンドのログに「not installed」）。`laoban-crd` をアップグレードすると 30 秒以内に保存が始まる |
| 月の途中で token 使用量をリセットしたい | その月の記録を削除してバックエンドを再起動する：`kubectl --context downstream -n laoban-system delete tokenusage tokens-YYYY-MM`（UTC の月）、続けて `kubectl --context downstream -n laoban-system rollout restart deploy/laoban` |
| アップグレード後、最初の `kubectl get rounds` がエラーになり、2 回目は成功する | kubectl の discovery キャッシュが古いままになっているだけなので、対応は不要です |

## アップグレード

1. **extension**：Rancher → Extensions → 右上の **⋮** → **Manage Repositories** → `laoban` 行の **⋮** → **Refresh**（Rancher は repo の一覧をキャッシュするため、Refresh しないと新しいバージョンが表示されません）。**Extensions** → **Installed** に戻り、laoban カード右上の **⋮** → **Upgrade** → 新しいバージョンを選択 → **Upgrade**、続けて画面上部の **Reload** をクリックします。
2. **バックエンド**：**先に** CRD をアップグレードし、次にバックエンドをアップグレードします（バージョンは新しいものに置き換えます）。

```bash
helm repo update
helm --kube-context downstream upgrade laoban-crd laoban/laoban-crd -n laoban-system --version <新しいバージョン>
helm --kube-context downstream upgrade laoban laoban/laoban -n laoban-system -f values-laoban.yaml --version <新しいバージョン>
```

進行中の意見のラウンド、未確認の計画、今月の token 使用量はクラスターに保存されているため、アップグレード後も残ります。各バージョンの注意事項は [Releases](https://github.com/yansheng133/laoban/releases) を参照してください。

## アンインストール

```bash
helm --kube-context downstream uninstall laoban -n laoban-system
helm --kube-context downstream uninstall laoban-crd -n laoban-system
kubectl --context local -n fleet-default delete gitrepo laoban-apps
```

extension は Rancher → Extensions → **Installed** → laoban のカード右上の **⋮** → **Uninstall** で削除します。生成されたアプリは `laoban-dev` とご自身の GitHub repo に残るため、不要になったら各自で削除してください。
