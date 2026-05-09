import type { ServerConfig } from "../types";

export function argsToStr(args: string[]): string {
  return args.map((a) => (a.includes(" ") ? `"${a}"` : a)).join(" ");
}

export function strToArgs(raw: string): string[] {
  return raw.match(/(?:[^\s"]+|"[^"]*")+/g)?.map((a) => a.replace(/^"|"$/g, "")) ?? [];
}

export function sameArgs(left: string[], right: string[]): boolean {
  return left.length === right.length && left.every((value, index) => value === right[index]);
}

export function serversToJson(servers: ServerConfig[]): Record<string, unknown> {
  const obj: Record<string, unknown> = {};
  for (const s of servers) {
    obj[s.name || `server_${Math.random().toString(36).slice(2, 6)}`] = {
      command: s.command,
      args: s.args,
      enabled: s.enabled,
      ...(s.env && Object.keys(s.env).length > 0 ? { env: s.env } : {}),
    };
  }
  return obj;
}

export function jsonToServers(obj: Record<string, unknown>): ServerConfig[] {
  const keys = Object.keys(obj);
  if (keys.length === 1 && keys[0] === "mcpServers" && obj["mcpServers"] && typeof obj["mcpServers"] === "object" && !Array.isArray(obj["mcpServers"])) {
    obj = obj["mcpServers"] as Record<string, unknown>;
  }
  return Object.entries(obj).map(([name, val]) => {
    const v = val as { command?: string; args?: string[]; env?: Record<string, string>; enabled?: boolean };
    return {
      name,
      command: v.command ?? "",
      args: v.args ?? [],
      env: v.env ?? {},
      description: "",
      cwd: "",
      lifecycle: null,
      stdioProtocol: "auto" as const,
      enabled: v.enabled !== false,
    };
  });
}
