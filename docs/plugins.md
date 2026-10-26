# CC Switch 页面插件（API v1）

第三方插件可以在「插件」入口添加一个或多个自定义页面。每个打开的页面由独立宿主进程显示在独立窗口中，主程序不加载第三方 HTML 或 JavaScript。页面是包含内联 CSS / JavaScript 的独立 HTML，可以由任意前端框架构建。插件无需改动 CC Switch 源码，也不需要编译 Rust。

## 安装与使用

1. 打开侧栏「插件」→「从文件夹安装」。
2. 选择包含 `cc-switch-plugin.json` 的文件夹。
3. 检查插件名称、版本、说明，点击「启用」并确认。
4. 点击「打开」启动独立插件窗口；主界面显示运行状态，并提供显示窗口、停止和重新打开操作。顶部页签可以切换插件声明的页面。

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

所有接口均为异步消息调用，返回 `Promise`，只能通过 `await` / `.then()` 获取结果；不提供同步读取、同步 IPC 或阻塞等待接口。供应商和用量查询、存储读写、插件安装管理及页面文件加载都在 Rust 后台线程执行，磁盘 I/O 和锁等待不占用主线程或异步执行器。业务查询使用独立只读数据库连接，不持有主程序共享连接的锁；数据库忙时快速返回 `PLUGIN_BUSY`，SQLite 查询执行预算为 2 秒。后续新增接口也必须遵守这一约束。

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

切换页面、返回列表或离开插件入口会停止对应子进程；停用、卸载或变更权限也会终止它。重新打开时创建新进程，保留已成功保存的插件存储。插件应自行提示保存状态并处理失败；没有后台运行、自动保存或导航拦截保证。请求超时为 10 秒；超时只结束 SDK 等待，不会强行中断已经开始的后台操作。宿主在操作完成前继续拒绝该页面的新数据请求；写入超时后应等待并重新读取核对，避免盲目重试。离开页面后，宿主丢弃迟到的响应。

## 边界

主程序启动自身可执行文件的专用插件宿主入口。该入口不初始化主程序数据库、供应商服务、托盘、单实例插件或通用命令桥。子进程只接收当前页面快照与上下文，通过独立 WebView 配置目录加载页面；Windows 使用独立 Job Object 管理进程树，Unix 使用独立进程组。子进程读取管道结束时自行退出，主程序退出或崩溃不会留下可继续工作的插件实例。

宿主使用自己的临时目录作为子进程工作目录，原生组件产生的相对缓存不会落到启动目录。子进程环境保留 Windows 系统路径变量（包括 `SystemDrive`、`ProgramData` 和 `ALLUSERSPROFILE`），确保系统缓存目录正常展开；不继承开发环境中的令牌变量。原生验收同时检查启动目录为空、子进程工作目录正确及系统路径变量可用。

插件 HTML 仅由子进程的 `cc-plugin` 协议返回，在 `sandbox="allow-scripts"` 的 iframe 内运行；不授予同源、弹窗、表单提交或顶层导航能力。独立 CSP 阻止网络、外部脚本和子框架。主程序不注册此协议，并用 CSP 禁止嵌入 frame。

子进程中的受信任桥校验消息来自当前 iframe 和不透明来源，主程序通过私有标准输入/输出管道接收有限方法。插件身份绑定在宿主会话，不接受消息提供的身份；每次操作检查启用状态与授权版本，业务读取保留字段白名单及权限校验。子进程不接收密钥、数据库文件路径或开发环境中的 API 令牌。没有通用 Tauri invoke、供应商写接口、Shell、文件系统或通用网络代理。

主程序每 250 毫秒检查存活状态；页面通过宿主发出的心跳挑战证明 JavaScript 仍能响应。首次启动最多等待 20 秒，运行后连续 10 秒无响应会终止插件进程树，主界面显示“插件无响应”，可重新打开。崩溃同样只结束该插件实例。界面、进程检查及终止逻辑均不依赖插件事件循环；系统休眠唤醒后重新计算心跳期限。

每个进程只允许一个未完成的数据请求；最多保留 4 个会话以容纳页面切换，主程序最多执行 4 个插件后台请求。管道采用长度前缀，子进程消息上限 128 KiB、响应上限 1 MiB；超过每秒 32 条消息会终止该实例，避免消息堆积。

这是故障隔离与受限 API，不是面向恶意本机代码的操作系统安全沙箱，也不提供整个系统的 CPU/内存硬配额。只安装信任的插件。没有插件商店、签名校验、自动更新、持久后台任务或原生模块支持。

存储位置为当前 CC Switch 配置目录的 `ui-plugins/plugin-<id>.json`，每个记录包含安装快照、启用状态与独立数据。这里使用本机文件，不属于现有数据库 Profile，也未接入云同步或备份导出。损坏条目在列表单独显示，不阻止其他插件加载，可通过卸载清理。

## 验证

```sh
pnpm typecheck
pnpm exec vitest run tests/components/PluginsPage.test.tsx tests/lib/uiPluginSdk.test.ts tests/lib/uiPluginRuntimeShell.test.ts
cargo test --manifest-path src-tauri/Cargo.toml --lib ui_plugins::
```

跨平台原生验收见 [Plugin Native Compatibility 工作流](../.github/workflows/plugin-compat.yml)：在 Windows WebView2 和 Linux WebKitGTK/Xvfb 中启动真实插件窗口，验证 SDK 与只读数据接口、死循环看门狗、崩溃恢复、撤权停用及主进程退出，并检查渲染子进程没有残留。每次运行上传 JSON 报告和子进程诊断日志。

验收入口只在显式启用 `test-hooks` 时编译，不进入正常发行构建。它使用临时配置和数据库，不读取用户的供应商数据。Windows 验收构建使用与发行版相同的无控制台子系统，覆盖私有管道与 Job Object 的实际行为。Linux 可在安装原生 WebView 与 Xvfb 依赖后运行：

```sh
cargo build --locked --manifest-path src-tauri/Cargo.toml --features test-hooks
dbus-run-session -- xvfb-run -a src-tauri/target/debug/cc-switch --plugin-compat-check "$PWD/plugin-compat-report.json"
```

实现参考：[Tauri capabilities](https://v2.tauri.app/security/capabilities/)、[MDN iframe sandbox](https://developer.mozilla.org/en-US/docs/Web/HTML/Reference/Elements/iframe)、[CSP sandbox](https://developer.mozilla.org/en-US/docs/Web/HTTP/Reference/Headers/Content-Security-Policy/sandbox)。这些机制仍需在各发布目标的原生 WebView 上验收。
