# laoban quickstart

[繁體中文](quickstart.md) · English · [日本語](quickstart.ja.md)

Follow this guide and in about 30 minutes you can build your first app from one sentence on your own Rancher. laoban is a community open-source project and is not covered by SUSE support; it is currently a preview (0.1.x) and still under heavy change.

## What you need

| Item | Description |
|---|---|
| Rancher 2.15 | Rancher Prime or the community edition. The extension is marked as supporting 2.15 only. The Rancher server must be able to reach `yansheng133.github.io` (the Helm repo) and `raw.githubusercontent.com` (the extension files) |
| A downstream cluster managed by Rancher | Runs the laoban backend and the generated apps. **The local cluster cannot be used** (Fleet puts the local cluster in a different workspace; laoban currently uses only `fleet-default`). Either x86-64 or ARM64 works (so far fully verified only on ARM64) |
| Outbound access | The downstream cluster must reach `api.anthropic.com` (or your local model), `api.github.com`, `ghcr.io` (the backend image) and `registry.suse.com` (the `bci/python` image the generated apps use); Fleet must be able to read your GitHub repo |
| `kubectl`, `helm` 3 | Able to operate on the cluster above; sections 5 and 7 also need access to the Rancher **local** cluster (for example, `/etc/rancher/rke2/rke2.yaml` on the Rancher host), or use the local cluster → **Import YAML** in the Rancher UI instead |
| GitHub account | A repo to hold the generated apps, and a token that can write only to that repo |
| Anthropic API key | Or a local model endpoint compatible with the Anthropic Messages API (for example, Ollama; see "Using a local model" in section 4) |

laoban calls the AI every time it generates a plan or changes code, and the cost is paid by your Anthropic account. The backend has a monthly token cap (default 2,000,000); when it is used up, laoban stops calling the AI.

> **Note**: Usage is counted per UTC month and stored in the cluster in `TokenUsage` resources (`kubectl --context downstream -n laoban-system get tokenusages`), so a backend restart does not reset it. It is laoban's own count, not Anthropic's bill: **be sure to also set a spend limit in the [Anthropic Console](https://console.anthropic.com/)**.

The commands below use `--context downstream` for the cluster that runs laoban and `--context local` for the Rancher local cluster. Replace them with your own kubeconfig contexts.

## 1. Install the extension in Rancher

The steps below follow the Rancher 2.15 screens.

1. Rancher top-left menu → **Extensions**.
2. Top right **⋮** → **Manage Repositories** → **Add Repository**:
   - Name: `laoban`
   - Target: **Helm Repository** (HTTP(S) URL pointing to a Helm chart repository index; this is the default)
   - Index URL: `https://yansheng133.github.io/laoban`
   - Click **Add Repository** at the bottom of the screen.
3. After saving, the screen stays on Apps → Repositories in the local cluster. Go back to **Extensions** from the top-left menu → **Available** tab, find the **laoban** card → **⋮** at the card's top right → **Install** → confirm the version (the latest, for example `0.1.22`) → **Install**. The dialog shows a yellow "untrusted authors" notice because laoban is a community project, not a Rancher-certified extension.
4. After installation, **Extensions changed - reload required** appears at the top; click **Reload** (or reload the page). **laoban** appears under **Global Apps** in the top-left menu.

If the Index URL does not open, use `https://raw.githubusercontent.com/yansheng133/laoban/gh-pages` instead; the content is the same.

## 2. Prepare GitHub

1. Create a new repo to hold the generated apps, for example `laoban-apps` (any name works; use yours in sections 4 and 5). **Check "Add a README file" when creating it** so that it has a `main` branch.
   - A public repo is fine for a trial: Fleet needs no extra read permission. But each app's `laoban.json` and README record the requests (masked) and the Rancher user IDs of the people who made changes, so **use a private repo for real use**. A private repo needs an additional Secret for Fleet to read it; see [deploy/m0/README.md](../deploy/m0/README.md).
2. Create a **fine-grained personal access token**:
   - Repository access: **Only select repositories**, selecting only the repo above.
   - Repository permissions: grant only **Contents: Read and write** (Metadata: Read-only is added automatically).
   - Save the token to a file, for example `./github-token`; do not paste it into commands.
3. Save the Anthropic API key to a file as well, for example `./anthropic-api-key`.

## 3. Create the namespaces and Secrets in the cluster

```bash
# Backend
kubectl --context downstream create namespace laoban-system
# Generated apps (enforced with Pod Security restricted)
kubectl --context downstream apply -f https://raw.githubusercontent.com/yansheng133/laoban/v0.1.22/deploy/m0/namespace-laoban-dev.yaml

# Read the key and token from files: they stay out of values and out of shell history
kubectl --context downstream -n laoban-system create secret generic laoban-anthropic --from-file=api-key=./anthropic-api-key
kubectl --context downstream -n laoban-system create secret generic laoban-github --from-file=token=./github-token
```

## 4. Install the backend

First look up the Rancher ID of the cluster that runs laoban (it starts with `c-m-`):

```bash
kubectl --context local get clusters.management.cattle.io \
  -o custom-columns=ID:.metadata.name,NAME:.spec.displayName
```

`clusterId` is required and, when `apps.url` is not set, is used to build the **Open app** URL (apps are opened through the Rancher proxy); the page does not need it to find the backend. Looking up the cluster ID requires access to the local cluster; alternatively, open that cluster in the Rancher UI, and the part after `/c/` in the address bar is the ID (for example, `c-m-xxxxxxxx`).

**Give the apps their own address (`apps.url`).** Without it, apps are opened through the Rancher proxy, on Rancher's own address: whoever opens an app runs the app's page with their Rancher login, and the page can call the Rancher API as them (as an administrator, when an administrator opens it). With `apps.url` set (`{app}` becomes the app name), each app gets an Ingress and opens on its own address, and the browser does not give it the Rancher login. You need:

- A wildcard DNS record pointing to the cluster's ingress controller, for example `*.apps.example.com`.
- For https, a wildcard certificate in `laoban-dev`, set as `apps.tlsSecretName`; set `apps.ingressClassName` if the class is not the default.
- Apps on their own address are no longer behind Rancher's permission check: anyone who can reach the address can open them. To require a login, attach your ingress controller's authentication with `apps.ingressAnnotations` (for example a Traefik middleware or ingress-nginx `auth-url`); the values are written into the apps repository, so no secrets.
- When the backend starts, it brings every app's Ingress in line with these settings in one commit: apps made earlier get one, a changed address is updated, and removing `apps.url` deletes them.
- Each app gets NetworkPolicies by default (`apps.networkPolicy`): the API accepts only the app's own web pods, and both reach only the app's API and DNS.

Create `values-laoban.yaml` (replace the `<...>` parts):

```yaml
github:
  repo: <your GitHub account>/laoban-apps
  existingSecret: laoban-github
  secretKey: token
anthropic:
  existingSecret: laoban-anthropic
  secretKey: api-key
config:
  # Anthropic model ID
  model: claude-sonnet-5-5
  # Monthly token cap
  tokenCap: 2000000
  targetNamespace: laoban-dev
  # Cluster ID looked up above
  clusterId: <c-m-xxxxxxxx>
# The apps' own address (strongly recommended, see below)
apps:
  url: https://{app}.apps.<your domain>/
  # Language of backend messages: en (default) or zh-Hant
  language: en
```

Install (CRDs first, then the backend):

```bash
helm repo add laoban https://yansheng133.github.io/laoban
helm repo update
helm --kube-context downstream install laoban-crd laoban/laoban-crd -n laoban-system --version 0.1.22
helm --kube-context downstream install laoban laoban/laoban -n laoban-system -f values-laoban.yaml --version 0.1.22
```

The published chart already contains the image digest for this version (`ghcr.io/yansheng133/laoban-server`), so you do not need to specify it.

### Using a local model

To use a local model compatible with the Anthropic Messages API (for example, Ollama) instead of the Anthropic API, change `values-laoban.yaml` like this:

```yaml
anthropic:
  existingSecret: ""   # no Anthropic API key needed
ai:
  baseUrl: http://<model server address>:11434   # must be reachable from Pods in the downstream cluster
  timeoutSeconds: 900                            # local models are slower
config:
  model: qwen3-coder:30b                         # the model name on the server
```

Keep the other fields. You do not need the `laoban-anthropic` Secret from section 3. `http://` is not encrypted, so use it only on an internal network.

Check that the backend is ready:

```bash
kubectl --context downstream -n laoban-system get pods
# READY must be 1/1
kubectl --context downstream get apiservice v1alpha1.api.laoban.yansheng133.github.io
# AVAILABLE must be True
```

## 5. Let Fleet deploy the generated apps

Create a GitRepo in the **Rancher local cluster**; Fleet deploys the apps under `apps/` in the repo to `laoban-dev`.

First look up the cluster name in Fleet (not the `c-m-` ID above):

```bash
kubectl --context local -n fleet-default get clusters.fleet.cattle.io
```

Download the two templates. In `fleet-gitrepo.yaml`, replace `https://github.com/<owner>/laoban-apps` with your repo and `clusterName: rke2` with the name you looked up, then apply them. **Do not change** the GitRepo's own name (`metadata.name: laoban-apps`): the page tracks deployments by that name:

```bash
curl -fsSLO https://raw.githubusercontent.com/yansheng133/laoban/v0.1.22/deploy/m0/fleet-gitrepo.yaml
curl -fsSLO https://raw.githubusercontent.com/yansheng133/laoban/v0.1.22/deploy/m0/fleet-gitreporestriction.yaml
# After editing fleet-gitrepo.yaml:
kubectl --context local apply -f fleet-gitreporestriction.yaml
kubectl --context local apply -f fleet-gitrepo.yaml
```

> **Note**: `fleet-gitreporestriction.yaml` restricts the entire `fleet-default` workspace: once applied, every GitRepo there can deploy only to `laoban-dev`. If `fleet-default` already has other GitRepos (`kubectl --context local -n fleet-default get gitrepos`), **do not apply this file for now**; apply only `fleet-gitrepo.yaml`. See [deploy/m0/README.md](../deploy/m0/README.md) for details.

## 6. Start using it

Switch the UI language in the user menu at the top right of Rancher → Preferences (繁體中文, English, 日本語). The words below are those of the English UI.

1. Rancher top-left menu → **Global Apps** → **laoban** → **Overview**. The page finds the backend on its own (a few seconds the first time).
2. On the left, describe the app you want in one sentence, for example "A simple message board where you can add and delete messages", and click **Generate plan**.
3. Review the AI's plan and 3D architecture diagram; if it looks good, click **Confirm and deploy**. The code is written to GitHub and then deployed by Fleet; when it is ready, click **Open app**.
4. Afterwards, open the app from **Apps**: you can edit the code by hand, compare or restore versions, or start a round of opinions so the team can comment in the 3D diagram; the AI consolidates the comments into one change plan, which is released only after everyone confirms.

**Hide menu** at the top left of the page collapses Rancher's side menu to leave more room for the workspace.

## 7. Let other people use it

- Rancher administrators can use it right away.
- Other people: those with the built-in **edit** role in `laoban-dev` can use it, and those with **view** can look. They usually cannot read Fleet's status; the page then checks the Deployments in `laoban-dev` to tell when a new version is running, so no extra permission is needed.
- If you manage access with Rancher Projects, apply the Project roles in the Rancher **local** cluster (use a kubeconfig that can manage the local cluster, for example `/etc/rancher/rke2/rke2.yaml` on the Rancher host; or paste this file into the local cluster → **Import YAML** in the Rancher UI), then assign members **laoban user** or **laoban viewer**:

```bash
kubectl --context local apply -f https://raw.githubusercontent.com/yansheng133/laoban/v0.1.22/deploy/rancher/roletemplate.yaml
```

  See [deploy/rancher/README.md](../deploy/rancher/README.md) for details.

## Troubleshooting

| Symptom | What to check |
|---|---|
| The laoban card on the Extensions page has a red exclamation mark, and laoban is not in the top-left menu | Whether the Rancher server can reach `raw.githubusercontent.com`; the URL in the "Failed to load script" message in the browser console; whether the extension is 0.1.18 or later (0.1.17 has this problem) |
| The page says "No laoban backend was found…" | AVAILABLE in `kubectl --context downstream get apiservice v1alpha1.api.laoban.yansheng133.github.io`; whether you have edit/view on `laoban-dev` or the laoban user/viewer role |
| Generating a plan fails and mentions the token or spend limit | Your quota in the Anthropic Console; the backend's monthly token cap (`config.tokenCap`) |
| Writing to GitHub fails | Whether the token has only Contents: Read and write on that one repo, and whether the repo has a `main` branch; backend log: `kubectl --context downstream -n laoban-system logs deploy/laoban` |
| Deployment never finishes | Status of `laoban-apps` in Rancher → Continuous Delivery → Git Repos; whether the Pod is blocked by Pod Security; whether the downstream cluster can pull `registry.suse.com/bci/python` |
| The new GitRepo shows no resources | Normal: the repo has no `apps/` yet; it appears after the first app is deployed |
| Generating a plan fails with a model "not found" error | Whether the model ID in `config.model` is correct; for a local model, download it on the server first |
| Rounds of opinions show a message asking an administrator to upgrade the laoban-crd chart first | `laoban-crd` is not installed, or its version is older than the backend |
| After a backend restart the token usage starts from 0, or plans not yet confirmed are gone | `laoban-crd` is older than 0.1.19 (the backend log says "not installed"). After upgrading `laoban-crd`, saving starts within 30 seconds |
| You want to reset the token usage in the middle of a month | Delete that month's record and restart the backend: `kubectl --context downstream -n laoban-system delete tokenusage tokens-YYYY-MM` (UTC month), then `kubectl --context downstream -n laoban-system rollout restart deploy/laoban` |
| After an upgrade, the first `kubectl get rounds` reports an error and the second one works | kubectl's discovery cache is still stale; no action needed |

## Upgrade

1. **Extension**: Rancher → Extensions → **⋮** at the top right → **Manage Repositories** → **⋮** on the `laoban` row → **Refresh** (Rancher caches the repo list; without Refresh the new version does not show). Back in **Extensions** → **Installed** → **⋮** at the top right of the laoban card → **Upgrade** → choose the new version → **Upgrade**, then click **Reload** at the top.
2. **Backend**: upgrade the CRDs **first**, then the backend (use the new version):

```bash
helm repo update
helm --kube-context downstream upgrade laoban-crd laoban/laoban-crd -n laoban-system --version <new version>
helm --kube-context downstream upgrade laoban laoban/laoban -n laoban-system -f values-laoban.yaml --version <new version>
```

Running rounds of opinions, plans not yet confirmed and this month's token usage are stored in the cluster and remain after the upgrade. See [Releases](https://github.com/yansheng133/laoban/releases) for notes on each version.

## Uninstall

```bash
helm --kube-context downstream uninstall laoban -n laoban-system
helm --kube-context downstream uninstall laoban-crd -n laoban-system
kubectl --context local -n fleet-default delete gitrepo laoban-apps
```

Remove the extension in Rancher → Extensions → **Installed** → **⋮** at the top right of the laoban card → **Uninstall**. The generated apps remain in `laoban-dev` and in your GitHub repo; delete them yourself when you no longer need them.
