# laoban クイックスタート

[繁體中文](quickstart.md) · [English](quickstart.en.md) · 日本語

このガイドに沿って進めると、約 30 分で自分の Rancher 上に、一文から最初のアプリを作れます。laoban はコミュニティのオープンソースプロジェクトであり、SUSE のサポート対象外です。現在は試食版（0.1.x）で、頻繁に変更しています。

## 準備するもの

| 項目 | 説明 |
|---|---|
| Rancher 2.15 | Rancher Prime またはコミュニティ版。extension がサポート対象として示しているのは 2.15 のみです。Rancher server から `yansheng133.github.io`（Helm repo）と `raw.githubusercontent.com`（extension のファイル）に接続できる必要があります |
| Rancher が管理するクラスター 1 つ | laoban バックエンドと生成されたアプリを実行します。downstream クラスターでも local でも構いません。アーキテクチャは x86-64 と ARM64 のどちらでも使えます（現時点で完全に検証済みなのは ARM64 のみ） |
| `kubectl`、`helm` 3 | 上記のクラスターを操作できること。第 5 節と第 7 節では Rancher の **local** クラスターも操作できる必要があります（例：Rancher ホスト上の `/etc/rancher/rke2/rke2.yaml`）。または Rancher UI の local クラスター → **Import YAML** を使います |
| GitHub アカウント | 生成されたアプリを置く repo と、その repo にのみ書き込める token |
| Anthropic API key | または Anthropic Messages API 互換のローカルモデルのエンドポイント（例：Ollama。[charts/laoban/README.md](../charts/laoban/README.md) の「改用本機模型」（ローカルモデルを使う）の節を参照） |

laoban は計画を作成したりコードを変更したりするたびに AI を呼び出し、その費用はお使いの Anthropic アカウントに請求されます。バックエンドには月間 token 上限（デフォルト 2,000,000）があり、使い切ると AI を呼び出さなくなります。

> **注意**：現時点では、この使用量はバックエンドのメモリーにしか保存されず、**バックエンドが再起動する（アップグレード、Pod の再作成、ノードの再起動）と 0 から数え直します**。そのため支出を抑える保証にはなりません。**必ず [Anthropic Console](https://console.anthropic.com/) で支出上限を設定してください**。

以下のコマンドでは、`--context downstream` が laoban を実行するクラスター、`--context local` が Rancher の local クラスターを表します。ご自身の kubeconfig context に置き換えてください。

## 1. Rancher に extension をインストールする

以下は Rancher 2.15 の画面に沿って説明します。

1. Rancher 左上のメニュー → **Extensions**。
2. 右上の **⋮** → **Manage Repositories** → **Add Repository**：
   - Name：`laoban`
   - Target：**Helm Repository**（HTTP(S) URL pointing to a Helm chart repository index。デフォルトでこれが選ばれています）
   - Index URL：`https://yansheng133.github.io/laoban`
   - **Create** をクリックします。
3. 保存すると、画面は local クラスターの Apps → Repositories に移ります。左上のメニューから **Extensions** → **Available** タブに戻り、**laoban** を見つけて → **Install** → バージョン（最新。例：`0.1.18`）を確認 → **Install**。
4. インストール後にページを再読み込みします。**laoban** は左上のメニューの **Global Apps** に表示されます。

Index URL が開けない場合は、代わりに `https://raw.githubusercontent.com/yansheng133/laoban/gh-pages` を使ってください。内容は同じです。

## 2. GitHub を準備する

1. 生成されたアプリを置く新しい repo を作成します（例：`laoban-apps`）。`main` ブランチができるように、**作成時に「Add a README file」にチェックを入れてください**。
   - まずは公開 repo で試すことをおすすめします。Fleet に追加の読み取り権限が不要です。プライベート repo の場合は、Fleet の読み取り用 Secret を別途作成する必要があります。[deploy/m0/README.md](../deploy/m0/README.md) を参照してください。
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
kubectl --context downstream apply -f https://raw.githubusercontent.com/yansheng133/laoban/v0.1.18/deploy/m0/namespace-laoban-dev.yaml

# key と token はファイルから読み込み、values に含めず、shell history にも残さない
kubectl --context downstream -n laoban-system create secret generic laoban-anthropic --from-file=api-key=./anthropic-api-key
kubectl --context downstream -n laoban-system create secret generic laoban-github --from-file=token=./github-token
```

## 4. バックエンドをインストールする

まず、laoban を実行するクラスターの Rancher 上の ID を調べます（`c-m-` で始まります。local クラスターは `local` です）。

```bash
kubectl --context local get clusters.management.cattle.io \
  -o custom-columns=ID:.metadata.name,NAME:.spec.displayName
```

`clusterId` は「アプリを開く」の URL を組み立てるためだけに使います（アプリは Rancher のプロキシ経由で開きます）。ページがバックエンドを見つけるのには不要です。クラスター ID を調べるには local クラスターの権限が必要です。Rancher UI でそのクラスターを開き、アドレスバーの `/c/` の後ろの部分を見ても確認できます（例：`c-m-xxxxxxxx`）。

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
  # バックエンドのメッセージの言語：en（デフォルト）または zh-Hant
  language: en
```

インストールします（先に CRD、次にバックエンド）。

```bash
helm repo add laoban https://yansheng133.github.io/laoban
helm repo update
helm --kube-context downstream install laoban-crd laoban/laoban-crd -n laoban-system --version 0.1.18
helm --kube-context downstream install laoban laoban/laoban -n laoban-system -f values-laoban.yaml --version 0.1.18
```

公開されている chart には、このバージョンのイメージ digest（`ghcr.io/yansheng133/laoban-server`）がすでに設定されているため、別途指定する必要はありません。

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

2 つのテンプレートをダウンロードし、`<owner>` をご自身の GitHub アカウントに、`clusterName: rke2` を上で調べた名前に置き換えてから適用します。

```bash
curl -fsSLO https://raw.githubusercontent.com/yansheng133/laoban/v0.1.18/deploy/m0/fleet-gitrepo.yaml
curl -fsSLO https://raw.githubusercontent.com/yansheng133/laoban/v0.1.18/deploy/m0/fleet-gitreporestriction.yaml
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
- その他のユーザー：`laoban-dev` で組み込みの **edit** ロールを持つユーザーは利用でき、**view** を持つユーザーは閲覧できます。
- Rancher Project で管理する場合は、Rancher の **local** クラスターに Project ロールを適用し（local クラスターを管理できる kubeconfig を使います。例：Rancher ホスト上の `/etc/rancher/rke2/rke2.yaml`。または Rancher UI の local クラスター → **Import YAML** にこのファイルを貼り付けます）、メンバーに **laoban user** または **laoban viewer** を割り当てます。

```bash
kubectl --context local apply -f https://raw.githubusercontent.com/yansheng133/laoban/v0.1.18/deploy/rancher/roletemplate.yaml
```

  詳しくは [deploy/rancher/README.md](../deploy/rancher/README.md) を参照してください。

## トラブルシューティング

| 症状 | 確認すること |
|---|---|
| Extensions ページの laoban カードに赤い感嘆符が付き、左上のメニューに laoban がない | Rancher server から `raw.githubusercontent.com` に接続できるか。ブラウザーの console に出る「Failed to load script」の URL。extension が 0.1.18 以降か（0.1.17 にはこの問題があります） |
| ページに「laoban バックエンドが見つかりません」と表示される | `kubectl --context downstream get apiservice v1alpha1.api.laoban.yansheng133.github.io` の AVAILABLE。`laoban-dev` の edit／view、または laoban user／viewer ロールを持っているか |
| 計画の作成に失敗し、token や支出上限について表示される | Anthropic Console の利用枠。バックエンドの月間 token 上限（`config.tokenCap`） |
| GitHub への書き込みに失敗する | token にその repo の Contents: Read and write だけが付与されているか、repo に `main` ブランチがあるか。バックエンドのログ：`kubectl --context downstream -n laoban-system logs deploy/laoban` |
| デプロイがいつまでも完了しない | Rancher → Continuous Delivery → Git Repos → `laoban-apps` のステータス。Pod が Pod Security でブロックされていないか |
| 意見のラウンドに、管理者に laoban-crd chart のアップグレードを求めるメッセージが表示される | `laoban-crd` がインストールされていないか、バージョンがバックエンドより古い |
| アップグレード後、最初の `kubectl get rounds` がエラーになり、2 回目は成功する | kubectl の discovery キャッシュが古いままになっているだけなので、対応は不要です |

## アンインストール

```bash
helm --kube-context downstream uninstall laoban -n laoban-system
helm --kube-context downstream uninstall laoban-crd -n laoban-system
kubectl --context local -n fleet-default delete gitrepo laoban-apps
```

extension は Rancher → Extensions → Installed → laoban → Uninstall で削除します。生成されたアプリは `laoban-dev` とご自身の GitHub repo に残るため、不要になったら各自で削除してください。
