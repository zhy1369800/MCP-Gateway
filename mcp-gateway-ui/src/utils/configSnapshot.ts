import type { GatewayConfig, ServerConfig, SkillsConfig } from "../types";
import { serversToJson } from "./serverConfig";

export interface EditableConfigSnapshot {
  servers: ServerConfig[];
  listen: string;
  apiPrefix: string;
  transport: GatewayConfig["transport"];
  security: GatewayConfig["security"];
  skills: SkillsConfig;
}

export type EndpointTransportType = "sse" | "streamable-http";
export type ServerDropPosition = "before" | "after";

export interface ServerDragTarget {
  index: number;
  position: ServerDropPosition;
}

function stableSortValue(value: unknown): unknown {
  if (Array.isArray(value)) {
    return value.map((item) => stableSortValue(item));
  }
  if (value && typeof value === "object") {
    const sorted: Record<string, unknown> = {};
    Object.entries(value as Record<string, unknown>)
      .sort(([a], [b]) => a.localeCompare(b))
      .forEach(([key, nested]) => {
        sorted[key] = stableSortValue(nested);
      });
    return sorted;
  }
  return value;
}

export function createEditableConfigSnapshot(input: {
  servers: ServerConfig[];
  listen: string;
  apiPrefix: string;
  ssePath: string;
  httpPath: string;
  adminToken: string;
  mcpToken: string;
  skills: SkillsConfig;
}): EditableConfigSnapshot {
  return {
    servers: input.servers,
    listen: input.listen,
    apiPrefix: input.apiPrefix,
    transport: {
      sse: { basePath: input.ssePath },
      streamableHttp: { basePath: input.httpPath },
    },
    security: {
      admin: { enabled: input.adminToken !== "", token: input.adminToken },
      mcp: { enabled: input.mcpToken !== "", token: input.mcpToken },
    },
    skills: input.skills,
  };
}

export function createEditableConfigFingerprint(snapshot: EditableConfigSnapshot): string {
  return JSON.stringify(stableSortValue(snapshot));
}

export function buildServersJson(servers: ServerConfig[]): string {
  return JSON.stringify(serversToJson(servers), null, 2);
}

export function moveArrayItem<T>(
  items: T[],
  sourceIndex: number,
  targetIndex: number,
  position: ServerDropPosition,
): T[] {
  if (sourceIndex === targetIndex) {
    return items;
  }

  const next = [...items];
  const [movedItem] = next.splice(sourceIndex, 1);
  let insertIndex = targetIndex;

  if (sourceIndex < targetIndex) {
    insertIndex -= 1;
  }
  if (position === "after") {
    insertIndex += 1;
  }

  insertIndex = Math.max(0, Math.min(insertIndex, next.length));
  next.splice(insertIndex, 0, movedItem);
  return next;
}

export function stripIndexKeyedEntries<T>(entries: Record<string, T>): Record<string, T> {
  return Object.fromEntries(
    Object.entries(entries).filter(([key]) => !key.startsWith("idx:")),
  ) as Record<string, T>;
}

export function createMcpClientEntryJson(
  name: string,
  type: EndpointTransportType,
  url: string,
  authorizationToken?: string,
): string {
  const trimmedToken = authorizationToken?.trim() ?? "";
  const entry: Record<string, unknown> = {
    type,
    url,
  };

  if (trimmedToken) {
    entry.headers = {
      Authorization: `Bearer ${trimmedToken}`,
    };
  }

  return JSON.stringify({
    [name]: entry,
  }, null, 2)
    .split("\n")
    .slice(1, -1)
    .join("\n");
}
