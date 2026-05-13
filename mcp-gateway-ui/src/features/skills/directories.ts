import type { SkillDirectoryValidation } from "../../types";

export type SkillDirStatus = "idle" | "checking" | "valid" | "invalid" | "error";
export type SkillDirKind = "roots" | "whitelist";

export interface SkillDirectoryItem {
  id: string;
  path: string;
  status: SkillDirStatus;
  enabled: boolean;
}

export interface SkillGroup {
  id: string;
  name: string;
  items: SkillDirectoryItem[];
}

export function createSkillDirectoryItem(path = "", enabled = false): SkillDirectoryItem {
  return {
    id: `dir-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
    path,
    status: "idle",
    enabled,
  };
}

export function createSkillGroup(name = "", items: SkillDirectoryItem[] = []): SkillGroup {
  return {
    id: `group-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
    name,
    items,
  };
}


export function skillDirectoryStatusFromResult(result: SkillDirectoryValidation): SkillDirStatus {
  if (result.hasSkillMd) {
    return "valid";
  }
  return "invalid";
}
