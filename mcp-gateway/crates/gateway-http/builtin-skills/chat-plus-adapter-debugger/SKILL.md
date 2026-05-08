---
name: chat-plus-adapter-debugger
description: 为 Chat Plus 新框架编写、修复、审查站点适配脚本。用于这些场景：根据真实 CDP Network 请求/响应/DOM 样本生成 adapter；检查现有脚本是否符合新框架硬规则；定位 transformRequest / extractResponse / decorateBubbles / continueConversation 哪一环写错。
---

# Chat Plus Adapter Debugger

## 目标

产出或修复一段可直接粘贴到 Chat Plus 的站点适配脚本，并满足新框架硬规则：

1. 最外层必须 `return { ... }`
2. 必须包含 `meta`
3. 必须包含四个 hook：
   - `transformRequest`
   - `extractResponse`
   - `decorateBubbles`
   - `continueConversation`
4. `decorateBubbles` 必须使用 `ctx.helpers.ui.decorateProtocolBubbles(...)`
5. `continueConversation` 必须使用 `ctx.helpers.plans.dom(...)`
6. `continueConversation` 必须使用 `ctx.continuationText`
7. `extractResponse` 必须返回 `responseContentPreview`
8. `transformRequest` 要么使用 `ctx.helpers.buildInjectedText(...)` 改写请求，要么明确 `return null`
9. 输出抓取必须基于真实协议 / 真实响应，不接受用消息 DOM 反向抄正文代替 `extractResponse`
10. 输入实现允许两条路：
   - 优先请求层协议注入
   - 协议层因加密、签名、opaque body 或其他原因无法稳定改写时，退回 DOM 发送
11. 生成脚本时必须使用平台 helpers：JSON 解析、文本转换、协议注入、气泡装饰、续写 DOM plan 都不要手写替代实现

## 使用 chrome-cdp

准备页面调试前，先确认工具列表存在 `chrome-cdp`，并读取 `builtin://chrome-cdp/SKILL.md`。本 skill 的执行入口可直接代理常用 CDP 调试命令，也可用更贴近 Chat Plus 工作流的别名。

只有用户明确允许启动浏览器调试会话后，才使用浏览器调试命令。MCP Gateway 内置 `chrome-cdp` 默认通过原生 Chrome DevTools Protocol 启动或复用独立的持久 profile Chrome，不连接用户已经打开的 Chrome 调试端口。

常用别名：

```bash
capture start
capture clear
network search <filter>
network get <request-id>
network perf
```

等价 CDP 命令：

```bash
netclear
net <filter>
netget <request-id>
perfnet
html [target] [selector]
snap [target]
evalraw <target> <method> [json]
```

如果 `chrome-cdp` 启动或浏览器连接失败：

1. 运行 `stop`
2. 再运行 `open <url>` 或 `launch <url>`
3. 不要要求用户关闭已有 Chrome 或已有 DevTools 调试端口，除非用户明确要求连接既有浏览器

## 抓样本流程

按这个顺序抓样本。不要把 URL、Performance index 或页面 DOM 文本当作稳定 request id。

1. 打开 / 选中目标页面，并确认处于用户真实登录态和真实使用模式
2. 执行 `capture start` 或 `netclear`，让 CDP Network 从用户操作前开始记录
3. 告诉用户现在只需要在页面里手动发送一条唯一探针文本，例如 `CHATPLUS_ADAPTER_PROBE_YYYYMMDD_001`
4. 用户回页面手动发送这条探针文本
5. 等用户明确回复：`发好了`
6. 执行 `network search <关键词>` 或 `net <关键词>` 搜索 CDP Network 记录
7. 按 URL / method / status / MIME / timing / body 线索找真实发送请求
8. 复制 `net` 输出里的 CDP request id
9. 执行 `network get <request-id>` 查看请求体、响应体、状态码、content-type 和传输类型
10. 根据 `network get` 输出总结协议结构，不复述敏感 headers
11. 只有当 body 太长、输出被截断、需要离线解码二进制/长流式内容时，才追加 `--request-file` / `--response-file` 保存到临时文件后再读取
12. 如果首条消息创建了新会话或跳转，必须在新会话页重新执行 `capture start`，让用户发第二条 follow-up 样本

硬要求：

- 不要代用户发送首条探针消息
- 不要在用户说 `发好了` 前假设请求已经产生
- 不要把“让用户自己开 DevTools 自己找请求”当默认流程；如果已获许可，优先由 AI 用 `chrome-cdp` 直接看
- `netget` 输出可能包含 Authorization、Cookie、设备 ID、会话 ID；最终回答和中间总结都不要复述这些值

## Request Id 规则

允许的查询方式：

- 操作前先执行 `capture start` 或 `netclear`
- 操作后执行 `network search <url-fragment>`、`network search <api-path>`、`network search <method>`、`network search <mime>` 或其它从候选里来的关键词
- WebSocket 请求同样先用 `network search <关键词>` 定位，再用 `network get <request-id>` 读取 frames
- 如果 `network search` 没有结果，用 `network perf` 或 `perfnet` 辅助确认是否漏开 Network、页面是否跳转、请求是否发生在 worker / service worker

禁止的反推方式：

- 不要把 Performance URL、Performance index 或 DOM 文本当作稳定 request id
- 不要在没执行 `capture start` / `netclear` 的情况下期待 CDP Network 有用户操作前的历史记录
- 不要扩大到 console、storage、历史请求或页面气泡里猜 chat 请求
- 不要在未确认 request id 前读取 body 或生成 adapter

## 证据层

抓取必须按三层证据链推进：

1. CDP Network 层
   - 用 `network search <filter>` 搜索真实 request id
   - 用 `network get <request-id>` 补齐浏览器层原始请求体、响应体、状态码和 content-type
2. PerformanceResourceTiming 层
   - 用 `network perf` 查看发送后新增资源
   - Performance URL 只是候选线索，不是 request id
3. DOM 层
   - 用 `html` / `snap` 确认用户消息、AI 消息、输入框、发送按钮、模式开关和消息气泡选择器
   - DOM 文本不能替代 `extractResponse` 的真实响应协议

## URL 已知但 Body / Response 缺失

先分类，不要直接归因为站点没有发请求：

1. URL 是错的：常见误拿 `OPTIONS`、埋点、标题生成、历史同步、配置、心跳、模型列表、附件上传
2. URL 是对的，但 `netget` 输出被截断或 body 太长：才用 request id 和 `--request-file` / `--response-file` 保存到临时文件
3. URL 是对的，但不是普通文本：按真实 transport 解析；不能稳定改写请求时，`transformRequest` 返回 `null`
4. URL 是对的，但首条消息重建页面或会话：进入新页面后抓第二条样本
5. URL 是对的，但 CDP Network 没有历史：重新 `capture start` 后再让用户发送

`netget` 看到的是网络 body，不是 F12 Preview。网络 body 可能包含应用层 framing / transport envelope，必须按协议层次还原。

网络 body 解码顺序：

1. 先看 `netget` 里的 status、content-type、content-encoding、request/response headers，不要只看文件扩展名
2. 如果是 `application/json` 或纯文本，按 UTF-8 直接读
3. 如果是 SSE / NDJSON，按文本流逐行解析
4. 如果 UTF-8 打开时在 JSON 前后出现 `\0`、控制字符、`�`、``、`` 等字节，先检查是否是 length-prefixed JSON 帧
5. 如果 payload 是 protobuf / MessagePack / CBOR / 压缩内容，就按对应协议 decoder 处理；不要盲目删除前几个字节

## 模式差异处理

有些站点存在会明显改变请求结构、响应结构或消息 DOM 的模式开关，例如：

- 思考 / 深度思考
- 联网搜索 / 智能搜索
- 专家模式 / 快速模式
- 其他会影响消息渲染或发送链的开关

遇到这种情况时，必须先确认用户实际使用的是哪种模式，再决定抓样本和写 adapter。

硬要求：

- 不要默认把“无思考”样本当成“有思考”也可用
- 不要默认把“无搜索”样本当成“有搜索”也可用
- 不要只因为基础聊天能工作，就假设带模式开关时 DOM 结构不变
- 如果存在思考 / 深度思考模式，`extractResponse` 必须排除思考文本，只提取最终 assistant 正文和需要保留的协议块
- 如果页面存在思考组件，`decorateBubbles` 的目标选择器必须避开思考组件，不把思考组件纳入协议卡片渲染范围

默认策略：

- 如果用户明确会使用思考模式，就优先在思考模式开启时调试
- 如果用户明确会使用搜索模式，就优先在搜索模式开启时调试
- 如果用户会同时使用多种模式，优先按“最复杂、最接近真实使用”的组合抓样本
- 如果用户不使用这些模式，可以按当前关闭状态调试

## 硬规则

### 1. 只接受新框架写法

- 不要接受手写 `renderProtocolCard`、`getProtocolCardTheme`、`detectToolResultTone` 这类旧 UI 辅助函数
- 不要接受旧版“自己拼 details 卡片”的 `decorateBubbles`
- 不要接受 `continueConversation` 自己 `click()` / `dispatchEvent()` / `setTimeout()`
- 不要接受硬编码 `[CHAT_PLUS_...]` 协议标记
- 不要接受缺 `meta`、缺 `adapterName`、缺 `capabilities`
- 不要接受 `JSON.parse(bodyText)` 直接解析请求体，必须走 `ctx.helpers.json.parse(bodyText)`
- 不要接受自己拼接注入文本，必须走 `ctx.helpers.buildInjectedText(...)`
- 不要接受自己把 `ctx.bodyText` / `ctx.responseText` 转字符串，必须走 `ctx.helpers.text.toText(...)`
- `meta.contractVersion` 必须是整数，按当前 Chat Plus 项目范式优先用 `2`
- `meta.capabilities` 必须用对象写明四类能力，不要写成数组、字符串或空对象

### 2. 四个 hook 的职责不能变

- `transformRequest(ctx)`
  负责请求层注入。输入注入优先走协议层。请求可稳定改写时，必须用 `ctx.helpers.text.toText(ctx.bodyText)` 取文本、用 `ctx.helpers.json.parse(bodyText)` 解析 JSON、用 `ctx.helpers.buildInjectedText(...)` 生成注入文本。请求不可稳定改写时，必须保留 hook 并 `return null`。

- `extractResponse(ctx)`
  负责从真实响应里提取 AI 正文。输出抓取必须走真实协议 / 真实响应。不要从页面现成消息 DOM 倒推正文，当成响应提取。必须保留协议块，不要把 `toolCall` / `toolResult` / `codeMode` 过滤掉。必须返回 `responseContentPreview`。不要随意 `slice(...)`。

- `decorateBubbles(ctx)`
  负责隐藏用户注入块，把可见协议块变成统一卡片，并保留协议块之外的普通文本。必须直接走 `ctx.helpers.ui.decorateProtocolBubbles(...)`。

- `continueConversation(ctx)`
  负责返回 DOM 发送方案。必须直接走 `ctx.helpers.plans.dom(...)`。必须使用 `ctx.continuationText`。不能自己执行真实 DOM 副作用。第一次 send 默认用 `mode: "click"`，只有确认站点必须 Enter 时才改。

### 3. 协议块语义

- `toolCall` 默认只展示，不等于自动执行
- 真正可自动执行 / 手动执行的是 `codeMode`
- `codeMode` 卡片的手动运行按钮、卡片容器、源码节点，统一由平台 helper 负责，不让站点脚本重复造轮子

## 工作流程

### 模式 A：生成或修复 Adapter

按下面顺序工作：

1. 确认目标站点 URL
2. 拿到真实请求样本
3. 拿到真实响应样本
4. 如果是流式响应，拿到完整流式样本
5. 拿到用户消息、AI 消息、输入框、发送按钮的 DOM 信息
6. 先判断 `transformRequest` 能不能稳定改写
7. 再判断 `extractResponse` 怎么提取正文和协议块
8. 再给出 `decorateBubbles`
9. 最后给出 `continueConversation`
10. 自检是否符合全部硬规则

如果样本不足，先指出缺什么，不要装作信息足够。

### 模式 B：审查现有 Adapter

不要只看四个函数在不在。必须逐项检查：

1. `meta` 是否完整
2. `transformRequest` 是否使用 `ctx.helpers.buildInjectedText(...)` 或明确 `return null`
3. `extractResponse` 是否返回 `responseContentPreview`
4. `decorateBubbles` 是否直接使用 `ctx.helpers.ui.decorateProtocolBubbles(...)`
5. `continueConversation` 是否直接使用 `ctx.helpers.plans.dom(...)`
6. `continueConversation` 是否使用 `ctx.continuationText`
7. 是否还残留旧卡片函数或协议硬编码

输出顺序固定：

1. findings
2. 证据不足 / 假设
3. 总评

## 抓样本时必须确认的内容

### 请求侧

- 用户真实输入在哪个字段
- 如果是消息数组，最后一条 user 消息怎么定位
- 请求体是不是 JSON
- 是否有签名、加密、opaque body，导致请求层不能稳定改写
- 如果请求层不能稳定改写，是否应改为 DOM 发送链兜底

### 响应侧

- AI 正文在哪个字段
- 是否是 SSE / EventSource / WebSocket / 其他流式格式
- 协议块是混在正文里，还是单独字段
- 是否存在 answer / thinking / summary 多阶段
- 思考 / 搜索模式开启后，响应事件格式是否发生变化
- 思考字段、思考事件、reasoning delta、thinking block 必须和最终正文分开；不要把思考内容拼进 `responseContentPreview`
- `codeMode` begin/end 是否能完整保留
- 输出是否能直接从真实响应事件 / 响应体提取，而不是依赖页面已渲染 DOM

### DOM 侧

- 用户消息容器
- AI 消息容器
- 思考区块 / 搜索区块 / 引用区块是否插入到 assistant turn 内部
- 思考区块选择器必须单独识别，并从 `assistantSelectors` / 协议卡片装饰范围中排除
- 输入框
- 发送按钮，或 Enter 发送目标
- 输入框赋值后是否需要 `input` / `change`

## 生成脚本时的强制要求

### meta

必须包含 `contractVersion`、`adapterName`、`capabilities`。默认能力结构按真实样本填写，常见 JSON 请求体 + SSE 响应 + helper 卡片 + DOM 续写应写成：

```js
meta: {
  contractVersion: 2,
  adapterName: "...",
  capabilities: {
    requestInjection: "json-body",
    responseExtraction: "sse",
    protocolCards: "helper",
    autoContinuation: "dom-plan",
  },
},
```

字段语义：

- `requestInjection`: 请求层注入方式，例如 `"json-body"`；如果请求不可稳定改写，可写 `"none"`，但 `transformRequest` 必须明确 `return null`
- `responseExtraction`: 真实响应提取方式，例如 `"sse"`、`"ndjson"`、`"json"`、`"websocket"`、`"grpc-web"`、`"dom-fallback"`；优先真实协议，不要默认 DOM
- `protocolCards`: 必须是 `"helper"`，对应 `ctx.helpers.ui.decorateProtocolBubbles(...)`
- `autoContinuation`: 必须是 `"dom-plan"`，对应 `ctx.helpers.plans.dom(...)`

### transformRequest

- 请求可改时：
  - 这是输入侧的优先方案
  - JSON 请求体必须用 `ctx.helpers.json.parse(bodyText)` 解析，不要写 `JSON.parse(bodyText)`
  - 请求文本必须用 `ctx.helpers.text.toText(ctx.bodyText)` 获取，不要自己转字符串
  - 用 `ctx.helpers.buildInjectedText(...)`
  - 不要自己拼接注入字符串
  - 返回 `applied / bodyText / requestMessagePath / requestMessagePreview`
- 请求不可改时：
  - 常见原因包括签名、加密、protobuf/二进制、opaque body、不可稳定定位消息字段
  - 保留 hook
  - 直接 `return null`
  - 后续输入改走 DOM 发送链，不要硬改协议

标准请求体读取和注入模板：

```js
const bodyText = ctx.helpers.text.toText(ctx.bodyText);
if (!ctx.injectionText || !bodyText) return null;

const body = ctx.helpers.json.parse(bodyText);
if (!body) return null;

const originalText = slot.text;
const nextText = ctx.helpers.buildInjectedText(ctx.injectionText, originalText, ctx.injectionMode);
if (nextText === originalText) return null;
slot.apply(nextText);
```

### extractResponse

- 默认返回完整提取文本
- 只能基于真实协议 / 真实响应提取
- 返回 `responseContentPreview`
- 如果是 SSE / 流式响应，先按真实事件格式重组，再提取正文
- 如果响应里有 thinking / reasoning / thought / analysis 等思考字段或事件，必须过滤掉，只把最终 answer/content 文本放入 `responseContentPreview`
- 不要把协议块截断
- 不要把页面消息气泡的 `innerText` / `textContent` 当作主提取方案

标准响应体读取模板：

```js
const responseText = ctx.helpers.text.toText(ctx.responseText);
if (!responseText) return null;
```

### decorateBubbles

只能写成这种方向：

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

允许传：

- `normalizeUserText`
- `normalizeAssistantText`
- `beforeRenderUserNode`
- `beforeRenderAssistantNode`
- 用于排除思考组件的选择器 / 过滤逻辑

但不要自己重写整套卡片系统。

如果站点有思考组件：

- `assistantSelectors` 应只命中最终回答消息区域，不要命中思考容器
- 如果思考组件嵌在 assistant turn 内部，必须用 helper 支持的过滤/normalize/beforeRender 钩子避开它
- 不要把思考文本当作普通 assistant 文本进行协议块扫描
- 不要给思考组件渲染协议卡片、代码卡片或工具卡片

禁止：

- 自己操作 DOM 创建卡片
- 自己隐藏协议块
- 自己写 details / button / code card
- 绕过 `ctx.helpers.ui.decorateProtocolBubbles(...)`

### continueConversation

只能写成这种方向：

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

输入侧策略说明：

- 优先协议注入
- 协议注入不可稳定实现时，才退回这个 DOM 发送方案
- 不要明明能稳定改请求，还默认只做 DOM 发送
- `input.kind` 必须按真实 DOM 确认后写 `"textarea"` 或 `"contenteditable"`，不要看见输入框就猜
- 第一次生成 `send` 时优先 `mode: "click"`，不要默认 Enter；只有真实站点要求 Enter 且有证据时才用键盘发送

禁止：

- `btn.click()`
- `dispatchEvent(...)`
- `setTimeout(...)`
- `return { dispatched: true }`
- 不使用 `ctx.continuationText`

### 已踩坑写法纪律

这些不是风格偏好，是生成和审查 adapter 时必须拦截的错误：

| 检查点 | 正确做法 | 错误做法 |
| --- | --- | --- |
| JSON 解析 | `ctx.helpers.json.parse(bodyText)` | `JSON.parse(bodyText)` |
| 文本注入 | `ctx.helpers.buildInjectedText(...)` | 手写字符串拼接 |
| 文本获取 | `ctx.helpers.text.toText(ctx.bodyText)` | `String(...)`、`.toString()`、手动解码 |
| `decorateBubbles` | `ctx.helpers.ui.decorateProtocolBubbles({...})` | 自己操作 DOM 或重造卡片 |
| `continueConversation` | `ctx.helpers.plans.dom({...})` | 自己 `click()` / `dispatchEvent()` |
| `continueConversation.send` | 首次默认 `mode: "click"` | 没证据就默认 Enter |
| 输入框 kind | 先确认 `"textarea"` 或 `"contenteditable"` | 对号入座、凭经验猜 |
| 最外层 | `return { meta: {...}, transformRequest, extractResponse, decorateBubbles, continueConversation }` | 裸函数、对象字面量、漏 hook |
| 协议注入 | JSON 可解析且能稳定定位消息时优先协议注入 | 能改请求却直接退 DOM |

## 自检清单

输出脚本前必须自己检查：

- [ ] 最外层是 `return { ... }`
- [ ] 有 `meta`
- [ ] `meta` 有 `contractVersion`
- [ ] `meta` 有 `adapterName`
- [ ] `meta` 有 `capabilities`
- [ ] `meta.capabilities.requestInjection`、`responseExtraction`、`protocolCards`、`autoContinuation` 都已按真实实现填写
- [ ] `protocolCards` 是 `"helper"`
- [ ] `autoContinuation` 是 `"dom-plan"`
- [ ] 四个 hook 全部存在
- [ ] `transformRequest` 不是乱改请求
- [ ] `transformRequest` 用 `ctx.helpers.text.toText(ctx.bodyText)` 读取请求文本
- [ ] JSON 请求体用 `ctx.helpers.json.parse(bodyText)` 解析，没有直接 `JSON.parse(bodyText)`
- [ ] 注入文本用 `ctx.helpers.buildInjectedText(...)` 生成，没有手写字符串拼接
- [ ] 输入侧已按“优先协议注入，失败再退 DOM 发送”判断
- [ ] `extractResponse` 返回 `responseContentPreview`
- [ ] 有思考 / reasoning 字段时，`extractResponse` 已排除思考文本，只保留最终正文和协议块
- [ ] 输出正文来自真实协议 / 真实响应，不是页面 DOM 抄取
- [ ] `decorateBubbles` 直接使用平台 helper
- [ ] 有思考组件时，`decorateBubbles` 的选择器 / 过滤逻辑已避开思考组件
- [ ] `continueConversation` 直接使用平台 helper
- [ ] `continueConversation` 使用 `ctx.continuationText`
- [ ] `continueConversation.send.mode` 首次默认 `"click"`，除非有证据必须 Enter
- [ ] `input.kind` 已从真实 DOM 确认为 `"textarea"` 或 `"contenteditable"`
- [ ] 没有旧卡片函数
- [ ] 没有硬编码 `[CHAT_PLUS_...]`

## 输出纪律

### 生成 / 修复 Adapter 时

最终先给：

1. 完整 JS 脚本
2. 极简说明
   - 输入走协议注入还是 DOM 发送
   - 如果没走协议注入，为什么不能稳定改协议
   - 响应是从哪个真实协议字段 / 事件提取
   - DOM 选择器
3. 风险点

### Review 时

最终先给 findings，不要先说“整体不错”。

## 禁止做法

- 不要输出伪代码
- 不要把旧框架写法说成“也可以”
- 不要只因为四个函数都在，就说脚本合格
- 不要凭站点名猜字段和选择器
- 不要在信息不足时拍脑袋生成高置信度脚本
- 不要把 `decorateBubbles` 降级成纯样式函数
- 不要把 `continueConversation` 写成真实 DOM 执行器
- 不要把输出抓取写成“从聊天气泡 DOM 读文本”
- 不要把思考 / reasoning 文本混入最终 `responseContentPreview`
- 不要让思考组件参与协议卡片装饰
- 不要在协议可稳定改写时，直接跳过协议注入只做 DOM 发送
- 不要在 `transformRequest` 里使用 `JSON.parse(bodyText)`、手写注入拼接、手动字符串转换
- 不要没确认 `textarea` / `contenteditable` 就写输入框 kind
- 不要没证据就把续写发送写成 Enter
- 不要输出临时页面注入代码
- 不要复述 `netget` 输出或临时文件里的敏感 header、cookie、token、设备 ID 或会话 ID

## 默认结论规则

- 如果一个脚本没用 `ctx.helpers.ui.decorateProtocolBubbles(...)`，就判定不符合新框架
- 如果一个脚本没用 `ctx.helpers.plans.dom(...)`，就判定不符合新框架
- 如果一个脚本在 `transformRequest` 里直接 `JSON.parse(bodyText)`，就判定需要改成 helper
- 如果一个脚本自己拼接注入文本，而不是用 `ctx.helpers.buildInjectedText(...)`，就判定需要修改
- 如果一个脚本没确认输入框 kind 或没证据就默认 Enter 发送，就判定存在续写风险
- 如果一个脚本缺 `meta`，就判定不符合新框架
- 如果一个脚本还带旧卡片函数，就判定需要重写
- 如果一个脚本的输出正文依赖页面消息 DOM 抓取，而不是协议 / 响应提取，就判定不符合框架预期
- 如果存在思考模式却把思考文本纳入 `responseContentPreview`，就判定响应提取错误
- 如果存在思考组件却把它纳入 `decorateBubbles` 的卡片渲染范围，就判定 DOM 选择器错误
- 如果请求体是可解析 JSON 且能稳定定位用户消息，却没有优先尝试协议注入，就判定不符合输入侧策略
- 如果请求不可改写，不是失败；只要 `transformRequest` 明确 `return null` 且 `continueConversation` 可用，就算符合框架
