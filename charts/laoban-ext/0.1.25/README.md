# laoban

Describe an app in one sentence. An AI agent proposes a plan and a 3D
architecture view first; code is written only after you confirm, committed to
GitHub and deployed by Fleet. A team can then comment on components in the 3D
view, and the AI converges the opinions into one change plan that is published
only after everyone who gave an opinion confirms it.

用一句話描述想要的應用，AI 先提出計畫與 3D 架構圖，確認後才寫程式、commit 到
GitHub、由 Fleet 部署。之後團隊可以在 3D 圖上提意見，由 AI 收斂成修改計畫，提意見
的人都確認後才發布。

**This extension needs the laoban backend.** Install it with Helm from the same
repository (`laoban/laoban-crd`, then `laoban/laoban`), as the Quickstart
describes:

- Quickstart (繁體中文): https://github.com/yansheng133/laoban/blob/main/docs/quickstart.md
- Quickstart (English): https://github.com/yansheng133/laoban/blob/main/docs/quickstart.en.md
- クイックスタート (日本語): https://github.com/yansheng133/laoban/blob/main/docs/quickstart.ja.md

The page finds the backend on the clusters you can see. The UI is in
Traditional Chinese, English and Japanese.

laoban is a community open source project (Apache-2.0) and is not supported by
SUSE. Issues: https://github.com/yansheng133/laoban/issues
