# laoban Helm repository

This branch is written by the `publish` workflow of
[yansheng133/laoban](https://github.com/yansheng133/laoban) on every release
tag. It is a Helm repository with:

- `laoban-ext`: the Rancher UI extension
- `laoban`: the backend
- `laoban-crd`: laoban's CRDs

Add it in Rancher (Extensions → ⋮ → Manage Repositories → Create) or with Helm:

```bash
helm repo add laoban https://yansheng133.github.io/laoban
```

Do not edit this branch by hand.
