# laoban 目前狀態

給接手的人（或其他 Claude 工作階段）快速知道 laoban 做到哪裡。隨每個版本更新；細節以 [m0-design.md](m0-design.md) 與 [Releases](https://github.com/yansheng133/laoban/releases) 為準。

**最後更新**：v0.1.18（2026-10-17）

## 一句話

Rancher UI extension：用一句話做出應用（AI 先給計畫與 3D 架構圖，確認後才寫程式，commit 到 GitHub，由 Fleet 部署），之後團隊在 3D 圖上提意見，AI 收斂成修改計畫，大家確認後才發布。試吃版（M0），社群專案，不在 SUSE 支援範圍內。

## 能做到的

| 範圍 | 狀態 |
|---|---|
| 安裝 | 公開 Helm repo `https://yansheng133.github.io/laoban`：extension（Rancher → Extensions）與後端兩個 chart（`laoban-crd`、`laoban`）。chart 已帶映像檔 digest。照 [quickstart.md](quickstart.md) |
| 做新應用 | 一句話 → 計畫、3D 架構圖 → 確認 → 程式寫進 GitHub → Fleet 部署 → 打開應用 |
| 應用的形狀 | 固定兩個元件：網頁加 API（Python，SUSE BCI 映像），一個 namespace，Pod Security restricted |
| 一起修改（意見收斂） | 一般與針對元件的意見、+1、回覆、改寫、撤回；草稿存在一輪裡（預設分享）；發起人改截止或立即收斂；AI 收斂成計畫；對計畫步驟留言；提意見的人都確認才發布；重啟後一輪還在；結束後歸檔到 `laoban-rounds` 分支，可看過去的輪次 |
| 版本 | 每一版是一個 commit；比較、還原；手動改程式；3D 圖拖曳存版 |
| 權限 | 後端是 Kubernetes aggregated API，用使用者自己的 Rancher 登入與 RBAC；內建 edit／view，或 Rancher Project 角色 laoban user／viewer；viewer 只能看 |
| 介面 | 繁體中文、English、日本語；後端訊息預設英文（`config.language: zh-Hant` 可改）；Rancher 側邊選單可收合 |
| AI | Anthropic API，或相容 Messages API 的本機模型（例如 Ollama）；送出前遮蔽敏感資料 |

## 已知限制

- 只標示支援 Rancher 2.15；只在 ARM64 上完整驗證過（x86-64 映像檔有建置）。
- 每月 token 上限只存在後端記憶體，**重啟後歸零**；花費上限要在 Anthropic Console 設。
- 一個 Rancher 只用一套後端；後端單一副本；計畫存在記憶體（1 小時）。
- 存在叢集裡的意見是原文，只有歸檔到 GitHub 的會遮蔽。
- 本機模型（例如 qwen3-coder）有時不照使用者的語言寫計畫。

## 下一步（候選）

- token 用量持久化。
- 第一階段：身分模擬、審查流程、版本 CRD、多人同時編輯（程式碼、3D 版面、意見、計畫）。

## 開發方式

- 程式碼由雲端的 Claude 寫、開 PR；Sam merge、打 tag。
- 每版用一份任務書（`laoban-cli-tasks/NN-*.md`）讓 Sam Mac 上的 CLI 部署與測試，回報 `NN-report.md`。
- 發布：打 `vX.Y.Z` tag → `image` 與 `publish` workflow → Helm repo 與 GitHub Release。
