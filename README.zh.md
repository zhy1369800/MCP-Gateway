# 本地 MCP Gateway

[English](./README.md) | [中文](./README.zh.md)

本地 MCP Gateway 是一个给 MCP 客户端使用的本地网关。

它把本机能力暴露成标准 MCP `SSE` 和 `Streamable HTTP` 接口，让其他 MCP 客户端通过一个入口调用。上游可以接普通 stdio MCP 服务、自定义 Skill、内置工具，也可以接桌面和浏览器自动化能力。

简单说，它会把你的电脑变成一个可控的 MCP 工具中心：客户端可以通过它读文件、改文件、执行命令、操作浏览器，或者调用你自己写的 Skill 工作流；同时配置、鉴权、目录边界、命令审批都集中在一个界面里管理。

![本地 MCP Gateway 界面](./image1.png)

## 它能做什么

- 把 MCP 服务暴露为 `SSE` 和 `Streamable HTTP`
- 把本地 stdio MCP 服务转换成其他客户端可访问的接口
- 在一个界面里统一管理多个 MCP 服务
- 固定暴露外部 Skill 和内置 Skill 两组 MCP 服务
- 自带文件读取、命令执行、多文件修改、浏览器控制、适配调试等工具
- 支持允许访问目录、命令策略、执行限制和人工确认
- Skill 形态比较省 token：核心说明放在小型 `SKILL.md` 文档里，客户端需要用时再读取完整说明

## 接口形式

假设你配置了一个名为 `<serverName>` 的 MCP 服务：

```text
SSE:  http://<监听地址>/api/v2/sse/<serverName>
HTTP: http://<监听地址>/api/v2/mcp/<serverName>
```

Skill 端点是固定的：

```text
外部 Skills: /api/v2/sse/__skills__
外部 Skills: /api/v2/mcp/__skills__
内置 Skills: /api/v2/sse/__builtin_skills__
内置 Skills: /api/v2/mcp/__builtin_skills__
```

如果配置了 `MCP Token`，客户端请求需要带上：

```text
Authorization: Bearer <你的_mcp_token>
```

## 基本用法

1. 打开应用，设置监听地址，例如 `127.0.0.1:8765`。
2. 添加本地 MCP 服务，比如 filesystem、Playwright 这类 stdio MCP。
3. 按需启用内置工具，或添加外部 Skill 目录。
4. 配置允许访问目录、命令确认规则和执行限制。
5. 启动网关。
6. 把生成的 `SSE` 或 `HTTP` 地址复制到你的 MCP 客户端。

示例：

```text
http://127.0.0.1:8765/api/v2/sse/playwright
```

## Skills 与内置工具

网关会暴露两类 Skill MCP 服务：

- `__skills__`：来自你添加的外部目录，每个 Skill 由一个 `SKILL.md` 描述。
- `__builtin_skills__`：网关自带的一组实用工具。

当前内置工具包括：

- `read_file`
- `shell_command`
- `multi_edit_file`
- `task-planning`
- `chrome-cdp`
- `chat-plus-adapter-debugger`（业务场景专用）
- `officecli`

![内置工具面板](./image2.png)

`chat-plus-adapter-debugger` 专门服务于 Chat Plus 站点适配流程，不是通用能力；做通用智能体时建议关掉它，避免上下文被业务规则占用。

这些能力可以让普通 MCP 客户端具备更接近智能体的工作流：查看项目、读取文档、修改代码、执行命令、验证结果、操作浏览器，并且所有调用仍然走标准 MCP 接口。

## 安全提示

部分 Skill 和内置工具可以执行命令、修改文件或控制本地应用。如果你要把网关暴露给本机以外的客户端，建议开启 `Admin Token` 和 `MCP Token`，并提前配置允许访问目录、确认规则和执行限制。

你需要自行确认并承担已授权命令和工具调用带来的结果。
