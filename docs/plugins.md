# CC Switch 页面插件（API v1）

第三方插件可以在「插件」入口添加一个或多个自定义页面。页面是包含内联 CSS / JavaScript 的独立 HTML，可以由任意前端框架构建。插件无需改动 CC Switch 源码，也不需要编译 Rust。

## 安装与使用

1. 打开侧栏「插件」→「从文件夹安装」。
2. 选择包含 `cc-switch-plugin.json` 的文件夹。
3. 检查插件名称、版本、说明，点击「启用」并确认。
4. 点击「打开」，使用顶部页签切换插件声明的页面。

安装会复制清单和页面内容为本地快照，原文件夹的后续修改不会自动执行。新插件默认停用，停用状态与插件数据在重启后保留。卸载会删除该插件的全部保存数据，但不会修改原始文件夹。当前不支持覆盖更新；安装新版本前请自行导出重要数据，再卸载旧版本。

## 插件结构

```text
my-plugin/
  cc-switch-plugin.json
  index.html
  about.html
```

```json
{
  "apiVersion": 1,
  "id": "org.example.my-plugin",
  "name": "My Plugin",
  "version": "1.0.0",
  "description": "Custom pages for CC Switch",
  "pages": [
    { "id": "notes", "title": "Notes", "entry": "index.html" },
    { "id": "about", "title": "About", "entry": "about.html" }
  ]
}
```

- `apiVersion` 必须为 `1`；未知字段会被拒绝，避免不支持的权限被默默忽略。
- 插件 ID 与页面 ID 以小写英文字母开头，只允许小写字母、数字、`-`、`.`，最多 64 字节；不允许 `..` 或末尾的 `.`。页面 ID 在插件内唯一。
- 插件名称和页面标题非空，最多 128 UTF-8 字节；版本非空，最多 64 字节；说明最多 1024 字节。建议使用反向域名 ID 和语义化版本。
- 一个插件声明 1–10 个页面。入口只能是当前文件夹中的普通 `.html` 文件，文件名主体仅包含英文字母、数字、`-`、`_`；不接受路径、符号链接或远程 URL。
- 每页最多 2 MiB，所有页面合计最多 8 MiB，最多安装 64 个插件。全部文件使用 UTF-8。
- 将脚本、样式和资源内联到 HTML；图片和字体使用 `data:` URL。不支持远程资源、外部 JS/CSS、动态模块加载、Worker 或 `eval`。React/Vue 等可预先构建为内联脚本，不能使用运行时模板编译器。

## 页面 API

宿主在页面脚本前注入 `window.CCSwitch`。无需 `<script src>`，实现位于 [`sdk/ui-plugin.js`](../sdk/ui-plugin.js)，类型位于 [`sdk/ui-plugin.d.ts`](../sdk/ui-plugin.d.ts)。

所有接口均为异步消息调用，返回 `Promise`，只能通过 `await` / `.then()` 获取结果；不提供同步读取、同步 IPC 或阻塞等待接口。供应商和用量查询、存储读写、插件安装管理及页面文件加载都在 Rust 后台线程执行，磁盘 I/O 和锁等待不占用主线程或异步执行器。后续新增接口也必须遵守这一约束。

```html
<!doctype html>
<meta charset="utf-8">
<h1>My plugin</h1>
<p id="result" role="status"></p>
<script>
  CCSwitch.ready.then(async (context) => {
    document.documentElement.lang = context.language;
    document.documentElement.dataset.theme = context.theme;
    const saved = await CCSwitch.getStorage();
    document.getElementById('result').textContent = saved?.message ?? 'Ready';
  }).catch(error => {
    document.getElementById('result').textContent = error.message;
  });
</script>
```

| API | 结果 |
| --- | --- |
| `CCSwitch.ready` | Promise，首次连接后返回上下文 |
| `CCSwitch.getContext()` | 返回 `{ apiVersion: 1, pluginId, pageId, language, theme }`；主题为 `light` / `dark` |
| `CCSwitch.getStorage()` | 返回该插件保存的 JSON；首次为 `null` |
| `CCSwitch.setStorage(value)` | 替换该插件保存的 JSON；最多 64 KiB（UTF-8 JSON 编码后），成功后 Promise 完成 |
| `CCSwitch.providers.list(options?)` | 读取已登记供应商的安全字段，需要 `providers.read` |
| `CCSwitch.usage.getStats(options)` | 读取用量汇总和趋势，需要 `usage.read` |

### 业务数据权限

清单可添加 `"permissions": ["providers.read", "usage.read"]`。不声明时默认为空，旧插件不会自动获得权限。启用时用户逐项确认授权；标题栏或列表的「权限」按钮可撤销或补充授权，停用插件会清除授权。权限设置由宿主控制，插件 SDK 不能自行申请、授予或修改权限。

每次读取都由 Rust 后端检查插件是否启用、是否声明且获得了对应权限。返回前再次检查授权版本；查询期间撤权、停用或重新安装会使该结果失效。页面已收到的数据无法被追回，宿主在自身授权界面变更权限后会卸载并重建插件页面。

`getContext()` / `ready` 的上下文还包含 `permissions`（当前获准权限列表），仅供界面展示，后端不信任插件提供的权限或身份。

```js
// 依次 await；宿主每个页面只允许一个未完成的数据请求。
const providers = await CCSwitch.providers.list({ appType: 'codex', offset: 0, limit: 20 });
const endDate = Math.floor(Date.now() / 1000);
const usage = await CCSwitch.usage.getStats({
  startDate: endDate - 7 * 86400,
  endDate,
  appType: 'codex'
});
```

- `providers.list` 返回 `{ items, total, offset, limit, source: 'registry' }`。每项仅有 `id`、`name`、`appType`、`category`；不读取或返回密钥、设置内容、端点、网站、备注及认证元数据。默认每页 100 项，范围 1–200，offset 范围 0–100000。数据仅限数据库已登记条目，不触发客户端配置导入，也不将数据库标记冒充当前实际启用状态。
- `usage.getStats` 要求 Unix **秒**级 `startDate < endDate`，最多 366 天，结束时间最多为当前时间后 24 小时。可选 `appType` 和 `providerName`；后者按显示名称精确匹配，沿用主用量页口径。
- 返回 `{ startDate, endDate, summary, trends }`。`summary` 包含请求数、实际处理 Token、输入/输出/缓存 Token、美元统计费用、成功率（0–100）和缓存命中率（0–1）。`trends` 各项含 `date`、`requests`、`tokens`、`costUsd`，一天以内按小时分组，其余按天分组。费用是数据库记录的统计值，不代表账单或余额。
- 查询复用主用量页的聚合逻辑，包含数据库中的代理、已导入会话及归档汇总数据，不返回请求正文、错误信息、会话 ID、模型明细或任何凭据。摘要与趋势为相邻的只读查询，后台同步期间可能有短暂差异，下一次刷新可更新。
- `PLUGIN_PERMISSION_DENIED` 表示未声明/未授权；`PLUGIN_ACCESS_CHANGED` 表示查询中授权变化；`PLUGIN_DISABLED` / `PLUGIN_UNAVAILABLE` 表示插件已停用/不可用；`PLUGIN_INVALID_QUERY` 表示范围或分页不合法；`PLUGIN_READ_FAILED` 表示数据读取失败。无权限时应清除旧数据显示授权提示，不应当作零用量。

一个插件的所有页面共用一份存储。写入使用原子替换，失败保留原值；停用插件不能读写存储。依次 `await` 存储调用，宿主拒绝同一页面并发提交存储请求。不要在这里保存 API 密钥或其他敏感信息。

主题和语言变化时，监听：

```js
addEventListener('cc-switch-context-changed', ({ detail }) => {
  document.documentElement.lang = detail.language;
  document.documentElement.dataset.theme = detail.theme;
});
```

切换页面、返回列表或离开插件入口会卸载页面，重新打开时重新运行。插件应自行提示保存状态并处理失败；没有后台运行、自动保存或导航拦截保证。请求超时为 10 秒；超时只结束 SDK 等待，不会强行中断已经开始的后台操作。宿主在操作完成前继续拒绝该页面的新数据请求；写入超时后应等待并重新读取核对，避免盲目重试。离开页面后，宿主丢弃迟到的响应。

## 边界

插件 HTML 由 `cc-plugin` 自定义协议返回，在 `sandbox="allow-scripts"` 的 iframe 内运行；不授予 `allow-same-origin`、弹窗、表单提交、顶层导航等能力。响应附带独立 CSP，阻止网络请求、外部脚本和子框架。主应用 CSP 只额外允许该插件协议作为 frame 来源，未放宽主应用的脚本策略。

宿主校验消息来自当前 iframe 的窗口和不透明来源，并只处理上述方法。插件 ID 取自宿主挂载记录，不接受消息指定的其他插件身份。业务数据仅通过带权限校验的字段白名单投影返回，没有通用 Tauri `invoke`、供应商写操作、凭据、文件系统、Shell 或通用网络代理接口。Tauri 的 IPC 初始化脚本仅注入顶层页面。

这是一种 WebView 页面隔离，不是独立进程或 CPU/内存配额；恶意或失控 JavaScript 仍可能影响 WebView 响应。只安装信任的插件。没有插件商店、签名校验、自动更新、后台任务或原生模块支持。

存储位置为当前 CC Switch 配置目录的 `ui-plugins/plugin-<id>.json`，每个记录包含安装快照、启用状态与独立数据。这里使用本机文件，不属于现有数据库 Profile，也未接入云同步或备份导出。损坏条目在列表单独显示，不阻止其他插件加载，可通过卸载清理。

## 验证

```sh
pnpm typecheck
pnpm exec vitest run tests/components/PluginsPage.test.tsx tests/lib/uiPluginSdk.test.ts
cargo test --manifest-path src-tauri/Cargo.toml --lib ui_plugins::
```

实现参考：[Tauri capabilities](https://v2.tauri.app/security/capabilities/)、[MDN iframe sandbox](https://developer.mozilla.org/en-US/docs/Web/HTML/Reference/Elements/iframe)、[CSP sandbox](https://developer.mozilla.org/en-US/docs/Web/HTTP/Reference/Headers/Content-Security-Policy/sandbox)。这些机制仍需在各发布目标的原生 WebView 上验收。
