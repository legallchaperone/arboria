# 部署文档网站

网站是 `docs/` 里的 Astro + Starlight 项目，构建结果是纯静态文件（`docs/dist`），放在任何静态托管上都能用。正式地址 **https://arboria.cwx.world**。

## A. Cloudflare Pages（推荐，当前在用）

Pages 项目已经连到 GitHub 仓库，`arboria.cwx.world` 的 DNS CNAME 也已经有了。在 Pages 项目的 **Settings → Builds & deployments** 里填：

| 设置 | 值 |
| --- | --- |
| Production branch | `main` |
| Framework preset | `None`（或 Astro，下面几项以这里为准） |
| Root directory (advanced) | `docs` |
| Build command | `npm ci && npm run build` |
| Build output directory | `dist` |
| Environment variable（Production 和 Preview 都加） | `NODE_VERSION` = `22.23.4` |

说明：

- 输出目录 `dist` 是相对 root directory 的，也就是仓库里的 `docs/dist`。
- `NODE_VERSION` 和 `docs/.nvmrc` 一致；Cloudflare 的构建镜像也会读 `.nvmrc`，环境变量写明更稳。以后升级 Node 时两处一起改。
- 依赖版本全部锁在 `docs/package-lock.json`，`npm ci` 严格按锁文件安装。
- 构建不需要 Python：CLI 参考页（`src/content/docs/reference/cli.md`）是生成后提交进仓库的，GitHub Actions 的 `docs` workflow 检查它是不是最新的。
- 每个 PR / 非 main 分支会得到一个预览地址 `https://<分支>.<项目>.pages.dev`。

### 绑定域名

1. Pages 项目 → **Custom domains** → **Set up a custom domain** → 输入 `arboria.cwx.world` → 继续。
2. `cwx.world` 的 DNS 在同一个 Cloudflare 账户里时，Cloudflare 会自动确认已有的记录；需要手动的话，DNS 记录应该是：

   | 类型 | 名称 | 目标 | 代理 |
   | --- | --- | --- | --- |
   | CNAME | `arboria` | `<项目名>.pages.dev` | 已代理（橙色云） |

   注意：只在 DNS 里加 CNAME、不在 Pages 项目里添加自定义域名，会得到 522 错误；两步都要做。
3. 等状态变成 **Active**（证书签好，一般几分钟），打开 https://arboria.cwx.world 。

### 本地确认这套设置能构建

```bash
git clone https://github.com/legallchaperone/arboria.git /tmp/arb-site && cd /tmp/arb-site/docs
npm ci && npm run build      # 和 Pages 上一模一样
npx astro preview            # http://localhost:4321
```

## B. GitHub Pages（备选，目前关着）

私有仓库要付费计划才能用 GitHub Pages（或者把仓库设成公开）。`.github/workflows/pages.yml` 已经写好，但有 `if: false`，而且只能手动触发，所以现在不会运行也不会失败。

要启用：

1. 仓库 **Settings → Pages → Build and deployment → Source** 选 **GitHub Actions**。
2. 删掉 `.github/workflows/pages.yml` 里的 `if: false` 这一行；想每次推送 main 都部署，再在 `on:` 里加上

   ```yaml
   push:
     branches: [main]
     paths: ["docs/**"]
   ```

3. **Settings → Pages → Custom domain** 填 `arboria.cwx.world`，勾上 **Enforce HTTPS**。`docs/public/CNAME` 已经带着这个域名。
4. DNS：`arboria` 的 CNAME 指向 `legallchaperone.github.io`（不要同时指向 Cloudflare Pages）。如果 DNS 在 Cloudflare，先把这条记录设成“仅 DNS”（灰色云），证书签好后再按需打开代理。

## 改了什么要注意

- 新增页面：放进 `docs/src/content/docs/`，并加进 `astro.config.mjs` 的 `sidebar`。
- 改了 `arb` 的命令行：`uv run python docs/scripts/gen_cli.py` 重新生成 CLI 参考页再提交（新命令要在脚本的 `SECTIONS` 里归类）。
- 截图在 `docs/src/assets/shots/`（WebP，每张都远小于 300 KB），构建时 Astro 会再生成不同宽度的版本。
- 网站不加载任何外部资源：没有网络字体、没有统计脚本，搜索（Pagefind）的索引也在 `dist/pagefind/` 里。
