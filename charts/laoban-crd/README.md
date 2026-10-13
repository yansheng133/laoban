# laoban-crd

laoban 的 CustomResourceDefinition：`ProjectPolicy`、`ChangeRequest`、`ArchitectureVersion`，API group `laoban.yansheng133.github.io`，版本 `v1alpha1`。

必須比 `laoban` chart 先裝；從 Rancher Apps 安裝 `laoban` 時，`catalog.cattle.io/auto-install` 會自動先裝相同版本的本 chart。

`keepOnUninstall`（預設 `true`）會在 CRD 加上 `helm.sh/resource-policy: keep`，`helm uninstall` 不會刪 CRD，也就不會連帶刪掉所有 laoban 物件。

設計理由、欄位說明與來源見 [charts/laoban/README.md](../laoban/README.md#crdlaoban-crd-chart)。

`ci/` 內的範例物件與 `test-crds.sh` 供 CI 對真正的 API server 驗證 schema 與 CEL 規則。
