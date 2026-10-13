# laoban Rancher UI extension

`extension/` 是 laoban 的 Rancher UI extension。側邊導覽列多一個頂層產品 **laoban**，點進去是試吃版（M0）的「總覽」頁：左邊輸入需求、看 AI 產生的計畫，右邊是計畫的 3D 架構圖，下方是狀態列；按「確認並部署」後，程式由後端寫進 GitHub、由 Fleet 部署，就緒後顯示「打開應用」。規格見 `docs/m0-design.md` 的「頁面（Rancher UI extension）」一節，頁面怎麼跟後端溝通見下方〈頁面與後端〉。

| 項目 | 值 |
|---|---|
| 目標 Rancher 版本 | Rancher Prime 2.15.2 |
| Extensions API | v3 |
| `@rancher/shell` | `3.0.13`（固定版本，不用 `^`） |
| Node.js | 24 以上（`@rancher/shell` 3.0.13 的 `engines` 是 `>=24.0.0`） |
| 套件管理 | Yarn 1（`@rancher/shell` 3.0.13 本身相依 `yarn@1.22.22`） |
| Extension 套件名稱 | `laoban-ext`（`pkg/laoban-ext`） |

laoban 沒有在 npm 上發佈任何套件，`laoban`、`@laoban` 都不是本專案的 npm 名稱。

## 目錄結構

骨架由官方產生器建立：

```bash
npm init @rancher/extension@3.0.12 laoban-ext -- --app-name extension
```

`npm init @rancher/extension` 實際執行的是 npm 上的 `@rancher/create-extension`（目前 `latest` 為 3.0.12），產生的根目錄 `package.json` 相依 `@rancher/shell ^3.0.13`，之後改成固定的 `3.0.13`。產生器預設會一併建立三個 GitHub workflow（`build-extension-charts.yml`、`build-extension-catalog.yml`、`build-extensions-test.yml`），但它們放在 `extension/.github/` 底下，GitHub 不會執行，而且發佈流程還沒有規劃，所以沒有收進來；建置改由專案根目錄的 `.github/workflows/ci.yml` 的 `extension` job 負責。

```
extension/
├── package.json              開發用 app（name: laoban，private）
├── yarn.lock
├── vue.config.js
├── jest.config.js            單元測試設定（pkg/*/utils/__tests__）
└── pkg/laoban-ext/           extension 本體
    ├── package.json          extension 中繼資料與 catalog annotation
    ├── index.ts              進入點：註冊產品、路由與額外語系
    ├── product.ts            頂層產品與側邊選單
    ├── config/laoban.ts      產品、頁面、路由名稱與額外語系
    ├── config/backend.ts     後端叢集 ID、namespace、Service 名稱、Fleet workspace、輪詢間隔
    ├── routing/extension-routing.ts
    ├── pages/Dashboard.vue   總覽頁（M0：需求、計畫、部署狀態）
    ├── components/ArchitectureView.vue  3D 架構圖（three.js）
    ├── utils/backend.ts      呼叫後端與 Steve 的 HTTP client
    ├── utils/bundle.ts       Fleet Bundle 就緒判斷（純函式）
    ├── utils/layout.ts       架構資料轉 3D 版面（純函式）
    ├── utils/__tests__/      上面三個檔案的單元測試
    └── l10n/                 en-us.yaml、zh-hant.yaml、ja.yaml
```

## 頁面與後端

頁面跑在 Rancher UI 裡，所有請求都用使用者自己的 Rancher 登入（瀏覽器帶 Rancher 的 session cookie）；extension 不保存也不轉送任何 token。POST 請求比照 Rancher 自己的做法，把 `CSRF` cookie 的值放進 `X-Api-Csrf` header。

| 用途 | 請求 | 經過 |
|---|---|---|
| 頁首資訊（模型、部署目標、程式庫、token 用量／上限） | `GET /api/config` | Rancher service proxy |
| 產生計畫 | `POST /api/plan` `{prompt, appName?}` | Rancher service proxy |
| 確認並部署 | `POST /api/apply` `{planId}` | Rancher service proxy |
| 部署狀態 | `GET /v1/fleet.cattle.io.bundles/fleet-default` | Steve（Rancher local 叢集） |

後端基底路徑是 Rancher 對 rke2 叢集上 `laoban-system/laoban` Service 的代理：

```
/k8s/clusters/c-m-26kbxk5f/api/v1/namespaces/laoban-system/services/http:laoban:8080/proxy
```

叢集 ID、namespace、Service 名稱與 port 都集中在 `pkg/laoban-ext/config/backend.ts`，換環境只改這個檔案。

- **等候時間**：產生計畫與部署都要呼叫 AI，通常 30～60 秒。請求進行中按鈕會停用，狀態列顯示已等候的秒數；超過 3 分鐘視為逾時。
- **錯誤**：後端回 `{"error": {"code", "message"}}` 時，直接顯示 `message`（後端負責寫成繁體中文）。連不到後端、Rancher 代理回的錯誤或格式不對的回應，顯示頁面自己的通用訊息。
- **部署狀態**：`/api/apply` 成功後顯示 commit 連結（新分頁開啟），接著每 5 秒讀一次 `fleet-default` 裡名稱以回應中 `fleet.bundlePrefix` 開頭的 Bundle。所有符合的 Bundle 都滿足下列條件才算就緒：`status.summary.desiredReady` 大於 0 且 `status.summary.ready` 等於它、`Ready` condition（如果有）為 `True`、`status.observedGeneration` 不小於 `metadata.generation`；Bundle 若帶有 `fleet.cattle.io/commit` label，還必須是這次的 commit。`status.summary.errApplied` 大於 0 時狀態列顯示部署錯誤，但仍繼續確認。10 分鐘內沒有就緒就停止，並提示到 Continuous Delivery 查看。
- **打開應用**：就緒後顯示「打開應用」，連到回應中的 `appUrl`（以 Rancher 的網址為基底）。只接受以單一 `/` 開頭的路徑，其他形式（其他網域、`//`、`javascript:`）一律不顯示連結。commit 連結只接受 `https://`。
- **3D 架構圖**：沿用 `web/viewer-prototype` 的畫法（three.js 0.160.0，打包進 extension，不從 CDN 載入），只做顯示、旋轉與縮放，沒有版本比較與拖曳。元件位置用後端給的 `pos`；有任何元件缺少或座標不合理時，改由頁面依層與上下層關係自動排列。元件數、名稱長度都有上限。系統設定「減少動態效果」時不使用慣性旋轉，只在畫面變動時重繪。瀏覽器不支援 WebGL 時顯示文字說明，元件清單在圖下方可以展開。

## 測試與檢查

```bash
cd extension
yarn test                  # Jest，測 utils/ 底下的純函式
yarn lint                  # @rancher/shell 的 ESLint 啟動器（第一次執行會安裝舊版 ESLint 工具鏈，需要網路）
```

Jest 與 Babel 都由 `@rancher/shell` 帶進來，沒有另外安裝測試套件。`yarn build-pkg` 本身會用 `fork-ts-checker` 檢查 `.ts` 與 `.vue` 的型別，型別錯誤會讓建置失敗。

## 建置

需要 Node.js 24 與 Yarn 1。官方文件只支援在 Linux 與 macOS 上開發 extension。

```bash
cd extension
npm install -g yarn        # 若尚未安裝 Yarn
yarn install --frozen-lockfile
yarn build-pkg laoban-ext
```

產出在 `extension/dist-pkg/laoban-ext-0.1.0/`，主檔為 `laoban-ext-0.1.0.umd.min.js`。`dist-pkg/`、`node_modules/` 都已列入 `.gitignore`，不要提交。

`.yarnrc` 是產生器預設的 `--frozen-lockfile true`，`yarn.lock` 對不上 `package.json` 時安裝會直接失敗。要更新相依套件時，先改 `package.json`，再用 `yarn install --no-default-rc` 重新產生 `yarn.lock`，並一起提交。

`three`（`0.160.0`，與 `web/viewer-prototype` 相同版本）寫在根目錄 `package.json` 的 `dependencies`，讓 Yarn 安裝、webpack 打包進 extension；`pkg/laoban-ext/package.json` 也列出同一個版本，標明 extension 本身用到它。型別定義 `@types/three` 是根目錄的 `devDependencies`。

## 在 Rancher 上開發與測試

測試 extension 需要 Rancher 的管理員帳號。

### 方法一：Developer load（載入本機建置結果）

1. 在 Rancher 右上角使用者頭像開啟 **Preferences**，在 **Advanced Features** 勾選 **Enable Extension developer features**。
2. 本機執行：

   ```bash
   cd extension
   yarn build-pkg laoban-ext
   yarn serve-pkgs          # 以 port 4500 提供 dist-pkg 的內容
   ```

   需要 Node.js 24 以上與 Yarn 1（`@rancher/shell` 3.0.13 的要求）。本機版本不夠時，可以跟 CI 一樣在 `registry.suse.com/bci/nodejs:24` 容器裡建置與執行。`serve-pkgs` 會聽所有網卡；在容器裡跑時用 `-p 127.0.0.1:4500:4500` 只開放給本機。

3. 在 Rancher 左上角選單開啟 **Extensions**，右上角三個點選 **Developer load**。
4. **Extension URL** 填入 `serve-pkgs` 印出的網址，例如 `http://127.0.0.1:4500/laoban-ext-0.1.0/laoban-ext-0.1.0.umd.min.js`，按 **Load**。瀏覽器要能連到這台機器的 4500 port。**網址一定要從 `http://` 開始**：少了的話 Rancher 不會擋，而是把它當成 Rancher 底下的相對路徑，載入時才失敗（console 出現 `Failed to load script from 'https://<Rancher>/dashboard/c/local/4500/...'`）。這時到 Extensions 移除它，再用完整網址重新載入。
5. 要在重新整理後保留，載入時勾選 **Persist extension by creating custom resource**。

### 方法二：本機開發伺服器

```bash
cd extension
API=https://<Rancher 位址> yarn dev
```

開啟 `https://127.0.0.1:8005`。開發伺服器會自動載入 `pkg/` 底下的 extension，改程式會即時更新。

### 方法三：Extension catalog（正式安裝）

正式安裝要先用 `yarn publish-pkgs` 打包成 Helm chart，或打包成 Extension Catalog Image（ECI），再在 Rancher 加入對應的 repository，從 **Extensions** 頁面安裝。本專案還沒有設定發佈流程，細節見官方文件「Publishing an Extension」。

## 相容性 annotation

`pkg/laoban-ext/package.json` 的 `rancher.annotations` 會在發佈時寫進 Helm chart 與 `index.yaml`：

| Annotation | 值 | 說明 |
|---|---|---|
| `catalog.cattle.io/ui-extensions-version` | `>= 3.0.0 < 4.0.0` | Extensions API v3。Rancher 2.10 起為必填，缺少時 Rancher 不會載入 extension |
| `catalog.cattle.io/rancher-version` | `>= 2.15.0-0 < 2.16.0-0` | 目前只在 Rancher 2.15 上驗證，範圍刻意不包含 2.16 |
| `catalog.cattle.io/display-name` | `laoban` | Extensions 頁面上的顯示名稱 |

## 多國語系

所有畫面上的文字都透過 `t()` 取翻譯字串，不直接寫在元件裡。翻譯檔放在 `pkg/laoban-ext/l10n/<語系>.yaml`，檔名就是語系代碼，由 `importTypes()` 自動註冊。使用者選的語系缺少某個字串時，會改用英文（`en-us`）。

| 檔案 | 語系 | Rancher 2.15 是否內建 |
|---|---|---|
| `en-us.yaml` | English | 是（預設語系） |
| `zh-hant.yaml` | 繁體中文（台灣用語） | 否 |
| `ja.yaml` | 日本語 | 否 |

Rancher 本身只內建 `en-us` 與 `zh-hans`（簡體中文），沒有繁體中文，也沒有日文。因此 `index.ts` 透過 `plugin.addLocale()` 把 `zh-hant`（繁體中文）與 `ja`（日本語）加進 Rancher 的語言選單。選了這兩個語系時，laoban 的畫面會顯示對應翻譯，Rancher 其他畫面則維持英文。laoban 被移除時，Rancher 會一併移除這兩個語系選項。若不想讓 laoban 改動語言選單，把 `config/laoban.ts` 的 `EXTRA_LOCALES` 清空即可，這時 `zh-hant.yaml` 與 `ja.yaml` 只有在其他來源提供同名語系時才會生效。

撰寫翻譯時用台灣常用的 IT 用語，產品名稱（Rancher、Kubernetes 等）保留英文。

試吃版（M0）只有繁體中文介面：`laoban.dashboard.*` 在 `zh-hant.yaml` 是正式文字，`en-us.yaml` 與 `ja.yaml` 先放同一組英文字串，日文翻譯之後再補。字串用 ICU 格式帶參數（例如 `{seconds}`），英文字串裡不要用單引號 `'`，ICU 會把它當成跳脫字元。

## 顯示 AI 產生的內容

AI 產生的文字（計畫摘要、步驟、元件名稱、連線標籤）一律以純文字顯示：在 template 裡用 `{{ }}` 插值，不用 `v-html`；3D 圖上的標籤用 `textContent` 放進 DOM，不用 `innerHTML`。產生器提供的 ESLint 設定已將 `vue/no-v-html` 設為 error。

如果將來真的需要顯示 HTML（例如 Markdown 轉成的內容），必須先經過 [DOMPurify](https://github.com/cure53/DOMPurify) 清理。`@rancher/shell` 已內建以 DOMPurify 實作的 `v-clean-html` directive，優先使用它，不要自行拼接 HTML。

## 參考資料

- 開始使用、產生器參數、建置與 Developer load：<https://extensions.rancher.io/extensions/next/extensions-getting-started>
- 頂層產品與路由：<https://extensions.rancher.io/extensions/next/usecases/top-level-product>
- 多國語系：<https://extensions.rancher.io/extensions/next/advanced/localization>
- `package.json` 設定與 annotation：<https://extensions.rancher.io/extensions/next/configuration>
- 版本相容性 annotation：<https://extensions.rancher.io/extensions/next/advanced/version-compatibility>
- Shell 版本對照：<https://extensions.rancher.io/extensions/next/support-matrix>
- 發佈（Helm chart、ECI）：<https://extensions.rancher.io/extensions/next/publishing>
- Rancher 內建語系：<https://github.com/rancher/dashboard/tree/master/shell/assets/translations>（只有 `en-us.yaml`、`zh-hans.yaml`）
- `@rancher/shell` 3.0.13 的 `engines`：`npm view @rancher/shell@3.0.13 engines`
