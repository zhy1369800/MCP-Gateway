import type { TFunction } from "../i18n";
import type { TerminalEncodingStatus } from "../types";

export function formatTime(value: string): string {
  const parsed = new Date(value);
  if (Number.isNaN(parsed.getTime())) {
    return value;
  }
  return parsed.toLocaleString();
}

export function runtimeDisplayValue(
  runtime: { installed: boolean; version: string | null } | undefined,
  loading: boolean,
  failed: boolean,
  t: TFunction,
): string {
  if (loading) {
    return t("runtimeChecking");
  }
  if (failed || !runtime) {
    return t("runtimeDetectFailed");
  }
  if (!runtime.installed) {
    return t("runtimeInstallPathHint");
  }
  return runtime.version?.trim() || t("runtimeDetectFailed");
}

export function terminalEncodingDisplayValue(
  terminal: TerminalEncodingStatus | undefined,
  loading: boolean,
  failed: boolean,
  t: TFunction,
): string {
  if (loading) {
    return t("runtimeChecking");
  }
  if (failed || !terminal || !terminal.detected) {
    return t("runtimeDetectFailed");
  }
  if (terminal.isUtf8) {
    if (terminal.codePage) {
      return t("runtimeUtf8CodePageValue").replace("{codePage}", String(terminal.codePage));
    }
    return t("runtimeUtf8Value");
  }
  if (terminal.autoFixOnLaunch) {
    if (terminal.codePage) {
      return t("runtimeNonUtf8AutoFixCodePageValue").replace(
        "{codePage}",
        String(terminal.codePage),
      );
    }
    return t("runtimeNonUtf8AutoFixValue");
  }
  if (terminal.codePage) {
    return t("runtimeCodePageValue").replace("{codePage}", String(terminal.codePage));
  }
  return t("runtimeNonUtf8Value");
}
