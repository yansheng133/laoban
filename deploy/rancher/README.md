# Rancher project roles for laoban

`roletemplate.yaml` defines two Rancher project roles:

| Role | Same as | Can do |
|---|---|---|
| laoban user (`laoban-user`) | the built-in edit role with `laoban-api-edit` | make and change apps, take part in rounds of opinions |
| laoban viewer (`laoban-viewer`) | the built-in view role with `laoban-api-view` | see apps, versions and rounds |

Without them only Rancher administrators (and whoever has the built-in edit / view role in the target namespace) can use laoban; a Project member's Rancher roles do not include laoban's API group.

## Install

1. Apply on the Rancher **local** cluster (RoleTemplates are Rancher management objects):

   ```bash
   kubectl --kubeconfig <local cluster kubeconfig> apply -f deploy/rancher/roletemplate.yaml
   ```

   Or in Rancher: switch to the local cluster → **Import YAML** → paste the file.
2. In **Cluster rke2 → Projects/Namespaces**, open the Project that holds `laoban-dev` → **Edit Config → Members** → add a user with the role **laoban user** or **laoban viewer**.
3. Rancher puts the rules into every namespace of that Project. The user can then use laoban as before.

## Remove

```bash
kubectl --kubeconfig <local cluster kubeconfig> delete roletemplate laoban-user laoban-viewer
```

Remove the role from the Project's members first; Rancher refuses to delete a RoleTemplate still in use.

The rules must stay the same as `charts/laoban/templates/aggregated-rbac.yaml`; CI compares them.
