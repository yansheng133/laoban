# 3D 架構視圖原型

laoban 第一階段的視覺化原型：把一個應用的網路、程式、資料三層畫成 3D，支援版本比較、細節層級與拖曳調整版面。資料是虛構的早餐店點餐系統。

## 看得到什麼

| 需求 | 原型裡的行為 |
|---|---|
| 程式、網路、資料庫的關係 | 三層平面：網路（Ingress、Service、NetworkPolicy）、程式（應用程式、模組）、資料（資料庫、資料表、佇列）；連線跨層畫出 |
| 預設高層次，可放大細節 | 細節分「概觀／服務／元件」三級，預設概觀。低層級時，被隱藏元件的連線會收合到它所屬的元件上 |
| 版本比較 | 勾選「比較」選基準版本：新增綠、移除紅（半透明、虛線）、修改橘、只動位置紫。右側列出所有差異，包含目前層級看不到的 |
| 審查提示 | 移除 NetworkPolicy、新增 Ingress、Service 改成 LoadBalancer／NodePort 時，顯示「需要人工審查」。規則與 `laoban-policy` 的「對外暴露」一致 |
| 舊版本唯讀、最新版可編輯 | 只有最新版本、而且不在比較模式時可以拖曳。調整後按「另存為新版本」，會產生新版本，舊版不變 |
| 三種語言 | 繁體中文、English、日本語 |
| 不信任 AI 產生的名稱 | 所有標籤都用 `textContent` 放進畫面，不經過 `innerHTML` |

位置變更歸類為「移動」，不算架構變更。

## 檔案

| 檔案 | 用途 |
|---|---|
| `index.html` | 頁面。three.js 0.160.0 從 jsDelivr 以 ES module 載入 |
| `graph-diff.mjs` | 版本差異、細節層級收合、另存版本；不依賴瀏覽器 |
| `sample-versions.mjs` | v1～v3 範例資料。v3 刻意移除資料庫的 NetworkPolicy |
| `graph-diff.test.mjs` | `node --test web/viewer-prototype/*.test.mjs` |

## 本機開啟

ES module 不能從 `file://` 載入，要經過 HTTP：

```bash
cd web/viewer-prototype && python3 -m http.server 8000
# 瀏覽器開 http://localhost:8000/
```

## 還沒做的

- 版本只存在頁面記憶體裡；之後接後端，新版本會成為 AI 下一輪的參考。
- 多人同時編輯、編輯鎖。
- 從實際的 manifest 與程式碼產生圖；目前是手寫的範例資料。
- 元件很多時的版面自動排列與效能。
