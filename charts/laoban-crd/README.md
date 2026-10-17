# laoban-crd

laoban 的 CustomResourceDefinition，API group `laoban.yansheng133.github.io`，版本 `v1alpha1`：

- `Round`：意見收斂的一輪，由後端寫入（試吃版 v0.1.15 起就要裝）。
- `Plan`：等待確認的計畫，由後端寫入，1 小時後刪除（v0.1.19 起）。
- `TokenUsage`：每個 UTC 月份的 token 用量，由後端寫入（v0.1.19 起）。
- `ProjectPolicy`、`ChangeRequest`、`ArchitectureVersion`：第一階段用。

要比 `laoban` chart 先裝、先升級。沒裝時後端仍會啟動，但意見收斂不能使用。

`keepOnUninstall`（預設 `true`）會在 CRD 加上 `helm.sh/resource-policy: keep`，`helm uninstall` 不會刪 CRD，也就不會連帶刪掉所有 laoban 物件。

設計理由、欄位說明與來源見 [charts/laoban/README.md](../laoban/README.md#crdlaoban-crd-chart)。

`ci/` 內的範例物件與 `test-crds.sh` 供 CI 對真正的 API server 驗證 schema 與 CEL 規則。
