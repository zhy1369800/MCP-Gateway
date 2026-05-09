import type { TFunction } from "../../i18n";
import type { ServerAuthState, ServerConfig } from "../../types";

export type ServerTestStatus = "idle" | "testing" | "success" | "failed" | "auth_required";
export type AuthChipTone = "idle" | "testing" | "success" | "failed" | "auth_required";

export interface ServerTestState {
  status: ServerTestStatus;
  message: string;
  testedAt?: string;
}

export function createEmptyAuthState(): ServerAuthState {
  return {
    status: "idle",
    browserOpened: false,
    sessionKey: "",
  };
}

export function authStateTone(state: ServerAuthState): AuthChipTone {
  if (state.status === "starting") return "testing";
  if (state.status === "connected" || state.status === "authorized") return "success";
  if (
    state.status === "auth_pending"
    || state.status === "browser_opened"
    || state.status === "waiting_callback"
  ) {
    return "auth_required";
  }
  if (
    state.status === "auth_timeout"
    || state.status === "auth_failed"
    || state.status === "launch_failed"
    || state.status === "init_failed"
  ) {
    return "failed";
  }
  return "idle";
}

export function authStateText(state: ServerAuthState, t: TFunction): string {
  if (state.status === "starting") return t("serverAuthStarting");
  if (state.status === "auth_pending") return t("serverAuthPending");
  if (state.status === "browser_opened") return t("serverAuthBrowserOpened");
  if (state.status === "waiting_callback") return t("serverAuthWaiting");
  if (state.status === "authorized") return t("serverAuthAuthorized");
  if (state.status === "connected") return t("serverAuthConnected");
  if (state.status === "auth_timeout") return t("serverAuthTimeout");
  if (
    state.status === "auth_failed"
    || state.status === "launch_failed"
    || state.status === "init_failed"
  ) {
    return t("serverAuthFailed");
  }
  return t("serverAuthIdle");
}

export function serverTestKey(index: number, server?: Pick<ServerConfig, "name">): string {
  const normalizedName = server?.name?.trim().toLowerCase();
  if (normalizedName) {
    return `name:${normalizedName}`;
  }
  return `idx:${index}`;
}

export function asErrorMessage(error: unknown): string {
  return String(error ?? "").replace(/^Error:\s*/, "").trim() || "Unknown error";
}
