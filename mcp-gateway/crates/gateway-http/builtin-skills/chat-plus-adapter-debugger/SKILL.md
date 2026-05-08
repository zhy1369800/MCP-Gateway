---
name: chat-plus-adapter-debugger
description: 为 Chat Plus 新框架编写、修复、审查站点适配脚本。用于这些场景：根据真实请求/响应/DOM 样本生成 adapter；检查现有脚本是否符合新框架硬规则；定位 transformRequest / extractResponse / decorateBubbles / continueConversation 哪一环写错；在用户明确允许调试 Chrome 页面后，配合 chrome-cdp 观察网络、DOM 和发送链，产出可直接粘贴的 site adapter。
---

# Chat Plus Adapter Debugger

## 目标

产出或修复一段可直接粘贴到 Chat Plus 的站点适配脚本。不要凭站点名、URL 或页面已渲染文本猜 adapter；必须基于真实请求、真实响应和真实 DOM 样本。

## 新框架硬规则

生成或审查 adapter 时必须满足：

1. 最外层必须 `return { ... }`
2. 必须包含 `meta.contractVersion`、`meta.adapterName`、`meta.capabilities`；按当前 Chat Plus 项目范式优先用整数 `2`
3. 必须包含四个 hook：`transformRequest`、`extractResponse`、`decorateBubbles`、`continueConversation`
4. `transformRequest` 要么使用 `ctx.helpers.buildInjectedText(...)` 稳定改写请求，要么明确 `return null`
5. `extractResponse` 必须从真实协议 / 真实响应提取正文，并返回 `responseContentPreview`
6. `decorateBubbles` 必须直接使用 `ctx.helpers.ui.decorateProtocolBubbles(...)`
7. `continueConversation` 必须直接使用 `ctx.helpers.plans.dom(...)`
8. `continueConversation` 必须使用 `ctx.continuationText`

禁止：

- 旧框架 UI：`renderProtocolCard`、`getProtocolCardTheme`、`detectToolResultTone`、手写 details 卡片
- `continueConversation` 自己 `click()`、`dispatchEvent()`、`setTimeout()` 或返回 `{ dispatched: true }`
- 硬编码 `[CHAT_PLUS_...]` 协议标记
- 把 `meta.contractVersion` 写成字符串，例如 `"2"`；它必须是整数
- 从页面消息 DOM 抄正文来冒充 `extractResponse`
- 请求可稳定改写时跳过协议注入，只做 DOM 发送

协议块语义：

- `toolCall` 默认只展示，不等于自动执行
- 真正可自动执行 / 手动执行的是 `codeMode`
- `codeMode` 卡片、按钮、源码节点由平台 helper 负责，站点脚本不要重复造轮子

## 使用 chrome-cdp

准备页面调试前，先确认工具列表存在 `chrome-cdp`，并读取 `builtin://chrome-cdp/SKILL.md`。

只有用户明确允许启动浏览器调试会话后，才使用 `chrome-cdp`。MCP Gateway 内置 `chrome-cdp` 默认通过 `chrome-devtools-axi` 启动独立的持久 profile Chrome，不连接用户已经打开的 Chrome 调试端口。

如果 `chrome-cdp` 启动、桥接或浏览器连接失败：

1. 运行 `stop`
2. 再运行 `open <url>` 或 `start`
3. 不要要求用户关闭已有 Chrome 或已有 DevTools 调试端口，除非用户明确要求连接既有浏览器

## Recorder 脚本

默认不要手写、解释、复制、粘贴或临时改写 recorder / CDP `eval` 注入内容。这个 recorder 是固定脚本，由 gateway 从二进制内置内容写入运行时目录；不要使用仓库开发路径。

强制约束：

- 安装、清空、读取 recorder 时，默认直接调用本 skill 的 recorder 动作，不要让 AI 手工转发 `eval`
- 允许的动作只有：`recorder install`、`recorder clear`、`recorder records`、`recorder records-full`、`recorder performance`
- gateway 会在内部运行固定脚本生成 AXI `eval`，再用 gateway 管理的 `chrome-cdp` 环境注入当前浏览器页面；不要在最终回答里展示这段 `eval`
- 如果 `eval` 内容过长，gateway 会自动分片注入以避开 Windows `npx.cmd` / `cmd.exe` 命令行长度限制；不要退回手工复制 eval
- 不要自己编写 `Runtime.evaluate`、`fetch` monkey patch、XHR monkey patch、WebSocket hook 或其它页面注入片段
- 不要把 recorder 的注入实现当作 adapter 输出；最终交付只能是 Chat Plus site adapter 和必要的极简说明
- 如果需要修改 recorder 行为，先修改 `scripts/recorder-command.mjs`，不要在对话里临时拼一段 CDP 注入脚本

首选用法：

```bash
recorder install
recorder records
recorder performance
recorder clear
recorder records-full
```

运行时脚本路径：

```bash
{{RECORDER_SCRIPT_PATH}}
```

底层脚本调试用法：

```bash
node "{{RECORDER_SCRIPT_PATH}}" install
```

这类 `node ... <action>` 命令只用于验证脚本会生成什么 AXI 命令；正常页面调试不要让 AI 手工复制它输出的 `eval`。

生成读取 recorder 记录的底层命令：

```bash
node "{{RECORDER_SCRIPT_PATH}}" records
```

生成读取完整 recorder 原始记录的命令：

```bash
node "{{RECORDER_SCRIPT_PATH}}" records-full
```

生成读取 Performance 增量候选的命令：

```bash
node "{{RECORDER_SCRIPT_PATH}}" performance
```

生成只清空 recorder / 重设 Performance 基线的命令：

```bash
node "{{RECORDER_SCRIPT_PATH}}" clear
```

使用方式：

1. 直接用本 skill 执行 `recorder install` / `recorder clear`
2. gateway 会内部生成并执行 `chrome-cdp eval`，不要手工转发 eval 内容
3. 每次让用户发送新样本前，必须运行 `install` 或 `clear`
4. 用户回复 `发好了` 后，先运行 `recorder records`
5. 如果 `records` 的 `candidates` 只有埋点、统计、标题、历史、配置或模型列表请求，立即运行 `performance`

脚本行为：

- `install` 首次安装 recorder；如果已安装，则复用并清空旧记录，不重复叠加 monkey patch
- `records` 返回全量增量记录摘要和通用候选评分，不按具体站点或 chat 关键词硬过滤
- `records-full` 返回完整原始记录，只在 `records` 摘要不足以判断协议结构时使用
- `performance` 返回发送后新增的全部资源条目和通用候选评分，不按具体站点或 chat 关键词硬过滤
- `performance` 的 URL 只是候选，下一步仍要用 `network --type ... --page ...` 找真实 `reqid`

如果运行时脚本路径不可用，先报告环境问题；不要在多个地方复制粘贴一份临时 recorder 源码。

## 一次命中流程

按这个顺序抓样本。不要从全量 Network 第一屏开始翻，也不要把 Performance URL 当成稳定的 `network-get` id。

1. 打开 / 选中目标页面，并确认处于用户真实登录态和真实使用模式
2. 用本 skill 执行 `recorder install`
3. 让用户手动发送唯一探针文本，例如 `CHATPLUS_ADAPTER_PROBE_YYYYMMDD_001`
4. 等用户回复 `发好了`
5. 用本 skill 执行 `recorder records`
6. 如果 `records` 的 `candidates` 命中真实 chat request body 和 response body，直接用这层样本
7. 如果 `records` 的 `candidates` 只命中埋点、标题、历史、配置、模型列表，继续用本 skill 执行 `recorder performance`
8. 根据 Performance 候选的 URL、initiatorType、startTime、duration、transferSize 收窄范围
9. 按下面的“Reqid 反推限制”选择唯一的 `network --type ... --limit 20 --page ...` 查询路径，分页找 `reqid`
10. 只有 Performance / recorder 候选指向 XHR 或 WebSocket 时，才查 `network --type xhr ...` 或 `network --type websocket ...`
11. 在 `network` 输出里按 URL / method / status / timing 找真实发送请求，复制 `reqid`
12. 用 `network-get <reqid> --request-file "<abs-path>\\chat-request.txt" --response-file "<abs-path>\\chat-response.txt"` 保存完整 body
13. 立刻列目录确认真实文件名；AXI 可能写成 `chat-request.network-request` 和 `chat-response.network-response`
14. 用 `shell_command` 读取保存的 body 文件，只总结协议结构，不复述敏感 headers
15. 如果首条消息创建了新会话或跳转，必须在新会话页重新安装 / 清空 recorder，让用户发第二条 follow-up 样本

## Reqid 反推限制

反推 `reqid` 必须按 recorder / Performance 候选配置收窄后查询，不要自由发挥。

允许的查询来源：

- recorder `records` 返回的候选：优先按记录里的 `kind`、`url`、`method`、`status`、`requestId` 交叉匹配
- recorder `performance` 返回的候选：按 `name`、`initiatorType`、`startTime`、`duration`、`transferSize` 交叉匹配

允许的 `network` 查询方式：

- 默认现代聊天请求：`network --type fetch --limit 20 --page 0`
- Performance / recorder 明确是 XHR：`network --type xhr --limit 20 --page 0`
- Performance / recorder 明确是 WebSocket：`network --type websocket --limit 20 --page 0`
- 只有当前页输出提示存在下一页时，才继续同一 `--type` 的 `--page 1`、`--page 2` 等后续页
- 每次只沿着当前候选对应的 `--type` 分页，不要在没有证据时轮询所有类型

禁止的反推方式：

- 不要从无类型的全量 `network` 第一屏开始翻
- 不要用 `network-get <url>`、Performance URL、Performance index 或 recorder `id` 当作稳定请求 id
- 不要调用原始 CDP `Network.*` / `Runtime.evaluate` 去绕过 AXI 的 `network --type ...` / `network-get <reqid>` 流程
- 不要扩大到 console、storage、DOM 文本或历史请求里猜 chat 请求
- 不要在未确认 `reqid` 前保存 body 或生成 adapter

判定规则：

- Performance URL 是候选线索，不是 `network-get` 的稳定 id
- 本 skill 覆盖 `chrome-cdp` 的通用建议；这里不要使用 `network-get <url>` 快速试探，必须回到 `network --type ... --page ...` 找 `reqid`
- 真正能稳定取 body / response 的优先钥匙是 `reqid`
- 不能只因为 recorder 没抓到，就说页面没有发聊天请求

## 证据层

抓取必须按三层证据链推进：

1. 页面 recorder 层
   - 记录 fetch / XHR / WebSocket / EventSource / sendBeacon 的增量
   - 用于快速排除埋点、资源、配置、心跳
   - 如果命中请求体和响应体，优先使用这层样本
2. PerformanceResourceTiming 层
   - 在用户发送前建立 baseline，用户发好后读取新增 entries
   - 能发现 recorder 漏掉但浏览器确实发出的请求
   - Performance index 不是 `network` 的 `reqid`
3. CDP Network 层
   - 用类型分页定位真实 `reqid`
   - 用 `network-get <reqid>` 补齐浏览器层原始请求体、响应体、状态码和 content-type
   - `network-get` 输出可能包含 Authorization、Cookie、设备 ID、会话 ID；最终回答和中间总结都不要复述这些值

常见 recorder 漏抓原因：

- `chrome-cdp eval` 运行在与页面主脚本不同的执行上下文
- 站点在注入前缓存了原始 `fetch` / `XMLHttpRequest` / SDK client 引用
- 请求由 worker、service worker、iframe、WebAssembly、RPC SDK 或内部 transport 发出
- 首条消息创建新会话、切换路由或重建应用实例
- 请求使用 Connect、gRPC-web、protobuf、二进制流、`application/connect+json`、`application/octet-stream`

## URL 已知但 body / response 缺失

先分类，不要直接归因为 `chrome-cdp` 失效：

1. URL 是错的：常见误拿 `OPTIONS`、埋点、标题生成、历史同步、配置、心跳、模型列表、附件上传
2. URL 是对的，但 `network-get` 输出省略：用 `reqid` 和 `--request-file` / `--response-file` 保存
3. URL 是对的，但不是普通文本：按真实 transport 解析；不能稳定改写请求时，`transformRequest` 返回 `null`
4. URL 是对的，但首条消息重建页面或会话：进入新页面后抓第二条样本
5. URL 是对的，但 recorder 抓不到 body：用 Performance 定位候选，再用 CDP Network 层补

如果保存文件仍没有 body，再判断是无 body、preflight、stream 未完成、二进制/opaque、WebSocket、worker/service-worker、redirect，还是工具捕获限制。

保存下来的 request / response 文件是网络 body，不是 F12 Preview。网络 body 仍可能包含应用层 framing / transport envelope。F12 能显示正常文本，是因为 DevTools 按 content-type、encoding、stream 类型和 preview 规则做了解码/折叠；这里必须自己按协议层次还原。

网络 body 解码顺序：

1. 先看 `network-get` 里的 status、content-type、content-encoding、request/response headers，不要只看文件扩展名
2. 如果是 `application/json` 或纯文本，按 UTF-8 直接读
3. 如果是 SSE / NDJSON，按文本流逐行解析
4. 如果 UTF-8 打开时在 JSON 前后出现 `\0`、控制字符、`�`、``、`` 等字节，先检查是否是 length-prefixed JSON 帧；常见形态是每段 JSON 前有 5 字节左右帧头，例如 `00 00 00 00 10 {"heartbeat":{}}`
5. 只有确认是 framed JSON，才剥离帧头或抽取 JSON payload，再按事件顺序解析
6. 如果 payload 不是 JSON 文本，而是 protobuf / MessagePack / CBOR / 压缩内容，就按对应网络协议 decoder 处理；不要盲目删除前几个字节

## 协议解析

按真实响应格式重组正文：

- SSE：按 `data:` 行解析，跳过 `[DONE]`，保留 delta / text / content 字段
- NDJSON：逐行 JSON parse，按事件顺序拼正文
- WebSocket：按 inbound frame 顺序拼 assistant 增量
- Connect/gRPC-web：常见为 length-prefixed message；先去掉二进制帧头，再按事件顺序解析 `set` / `append` / `done` 等操作。保存文件里如果长度字节被显示为 `�` 或刚好是 `{`、`"` 这类可见字符，不要用普通“找第一个 `{`”解析；只把真正以 `{` 后接 `"` 或 `}` 开始的 JSON 对象当 payload
- protobuf / MessagePack / CBOR / opaque 二进制：这些也是网络响应 body，但不是去掉帧头就能变成文本；不要硬写 JSON 提取。先判断是否有对应 decoder 或是否需要 DOM 发送，并继续寻找响应侧可解析协议

## 模式差异

有些站点的思考、搜索、专家、文件、长文本等模式会改变请求结构、响应结构或消息 DOM。抓样本前必须确认用户真实使用的模式。

- 如果用户会使用思考 / 搜索等复杂模式，按真实模式抓样本
- 基础聊天样本不能默认覆盖复杂模式
- 如果请求体有签名、加密、二进制 body 或不可解释 token，不要强行做协议注入；输入可退回 DOM 发送，但输出仍必须找真实响应协议

## 生成 adapter

### Chat Plus adapter ctx 契约

按当前 Chat Plus 项目源码，adapter hook 接收的 `ctx` 字段是稳定契约。生成或审查脚本时以这个表为准：

| 场景 | 字段 | 含义 |
| --- | --- | --- |
| `transformRequest` | `ctx.bodyText` | 当前请求体文本 |
| `transformRequest` | `ctx.injectionText` | Chat Plus 要注入到用户消息前的文本 |
| `transformRequest` | `ctx.injectionMode` | 注入模式，通常是 `"system"` 或 `"raw"` |
| `transformRequest` / `extractResponse` | `ctx.url` | 当前请求或响应对应的 URL |
| `extractResponse` | `ctx.responseText` | 当前响应体文本或已缓冲的响应预览文本 |
| `decorateBubbles` | `ctx.root` | DOM 快照根节点 |
| `decorateBubbles` / `extractResponse` | `ctx.protocol` | 当前 Chat Plus 协议标记配置 |
| `continueConversation` | `ctx.continuationText` | 需要回填到站点输入框的续写文本 |
| 全部 hook | `ctx.helpers` | Chat Plus 提供的解析、协议、DOM plan 和 UI helper |

`ctx.helpers.buildInjectedText(...)` 的源码签名是三参位置参数，返回注入后的字符串：

```js
const nextText = ctx.helpers.buildInjectedText(
  ctx.injectionText,
  originalText,
  ctx.injectionMode,
);
```

标准请求体读取和注入模板：

```js
const bodyText = ctx.helpers.text.toText(ctx.bodyText);
if (!ctx.injectionText || !bodyText) return null;

const originalText = slot.text;
const nextText = ctx.helpers.buildInjectedText(ctx.injectionText, originalText, ctx.injectionMode);
if (nextText === originalText) return null;
slot.apply(nextText);
```

标准响应体读取模板：

```js
const responseText = ctx.helpers.text.toText(ctx.responseText);
if (!responseText) return null;
```

### transformRequest

- 请求可稳定改写时，用 `ctx.helpers.buildInjectedText(...)`，返回 `applied / bodyText / requestMessagePath / requestMessagePreview`
- 请求不可稳定改写时，保留 hook 并 `return null`

### extractResponse

- 只能基于真实协议 / 真实响应提取
- 返回完整提取文本和 `responseContentPreview`
- 流式响应先按真实事件格式重组
- 不要截断协议块，不要从聊天气泡 DOM 读正文

### decorateBubbles

必须直接使用：

```js
decorateBubbles(ctx) {
  return ctx.helpers.ui.decorateProtocolBubbles({
    root: ctx.root || document,
    protocol: ctx.protocol,
    userSelectors: [...],
    assistantSelectors: [...],
  });
}
```

### continueConversation

必须直接使用：

```js
continueConversation(ctx) {
  return ctx.helpers.plans.dom({
    root: ctx.root,
    composerText: ctx.continuationText,
    input: {
      selectors: [...],
      kind: "textarea",
      dispatchEvents: ["input", "change"],
    },
    send: {
      mode: "click",
      selectors: [...],
      waitForEnabled: true,
      maxWaitMs: 2000,
    },
  });
}
```

## 审查 adapter

审查时先给 findings，不要先总结。逐项检查：

1. `meta` 是否完整，且 `contractVersion` 必须是整数，按当前 Chat Plus 项目范式优先用 `2`
2. 四个 hook 是否齐全
3. `transformRequest` 是否按当前 ctx 契约读取 `ctx.bodyText`
4. `transformRequest` 是否使用三参 `ctx.helpers.buildInjectedText(ctx.injectionText, originalText, ctx.injectionMode)` 或明确 `return null`
5. `extractResponse` 是否按当前 ctx 契约读取 `ctx.responseText`
6. `extractResponse` 是否基于真实响应并返回 `responseContentPreview`
7. `decorateBubbles` 是否直接使用 `decorateProtocolBubbles`
8. `continueConversation` 是否直接使用 `plans.dom` 并使用 `ctx.continuationText`
9. 是否残留旧卡片函数、旧 UI、硬编码协议标记或 DOM 抄正文

## 输出纪律

生成 / 修复 adapter 时，最终输出：

1. 完整 JS 脚本
2. 极简说明：输入走协议注入还是 DOM 发送；响应从哪个真实字段 / 事件提取；DOM 选择器
3. 风险点：哪些模式或协议还缺样本

最终回答禁止输出：

- recorder 源码、`eval "function(){...}"`、`Runtime.evaluate` 片段或任何 CDP 注入实现
- 临时 monkey patch 代码
- `network-get` 保存出来的敏感 header、cookie、token、设备 ID 或会话 ID

如果样本不足，明确指出缺什么，不要输出高置信度伪脚本。
