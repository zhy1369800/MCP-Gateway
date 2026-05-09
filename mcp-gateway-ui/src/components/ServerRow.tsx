import { useEffect, useState } from "react";
import { Check, Copy, Eye, EyeOff } from "lucide-react";
import type { TFunction } from "../i18n";
import type { ServerAuthState, ServerConfig } from "../types";
import { argsToStr, sameArgs, strToArgs } from "../utils/serverConfig";
import { formatTime } from "../utils/display";
import type { EndpointTransportType } from "../utils/configSnapshot";
import { authStateText, authStateTone, type ServerTestState } from "../features/servers/serverStatus";

export function ServerRow({
  server,
  onChange,
  onDelete,
  running,
  baseUrl,
  ssePath,
  httpPath,
  copied,
  onCopy,
  testState,
  authState,
  onTest,
  onReauthorize,
  onClearAuth,
  t,
}: {
  server: ServerConfig;
  onChange: (u: ServerConfig) => void;
  onDelete: () => void;
  running: boolean;
  baseUrl: string;
  ssePath: string;
  httpPath: string;
  copied: string | null;
  onCopy: (name: string, type: EndpointTransportType, url: string, key: string) => void;
  testState: ServerTestState;
  authState: ServerAuthState;
  onTest: () => void;
  onReauthorize: () => void;
  onClearAuth: () => void;
  t: TFunction;
}) {
  const sseUrl  = `${baseUrl}${ssePath}/${server.name}`;
  const httpUrl = `${baseUrl}${httpPath}/${server.name}`;
  const showLinks = running && server.enabled && server.name.trim();
  const isTesting = testState.status === "testing";
  const statusText = testState.status === "testing"
    ? t("serverTestTesting")
    : testState.status === "success"
      ? t("serverTestSuccess")
      : testState.status === "auth_required"
        ? t("serverTestAuthRequired")
      : testState.status === "failed"
        ? t("serverTestFailed")
        : t("serverTestIdle");
  const statusTitle = testState.testedAt
    ? `${statusText} · ${formatTime(testState.testedAt)}${testState.message ? `\n${testState.message}` : ""}`
    : (testState.message || statusText);
  const authText = authStateText(authState, t);
  const authTitleParts = [authText];
  if (authState.lastSuccessAt) {
    authTitleParts.push(`${t("serverAuthLastSuccess")} ${formatTime(authState.lastSuccessAt)}`);
  }
  if (authState.lastError) {
    authTitleParts.push(authState.lastError);
  }
  if (authState.authorizeUrl) {
    authTitleParts.push(authState.authorizeUrl);
  }
  const authTitle = authTitleParts.join("\n");
  const showAuthActions = !!authState.adapterKind || authState.status !== "idle" || !!authState.lastSuccessAt;

  // 环境变量数组形式（方便渲染）
  const envEntries = Object.entries(server.env);
  const [visibleEnvValues, setVisibleEnvValues] = useState<Record<string, boolean>>({});
  // 保留编辑中的原始文本，避免每次按键都把空格/引号格式化掉。
  const [argsDraft, setArgsDraft] = useState(() => argsToStr(server.args));
  const [isEditingArgs, setIsEditingArgs] = useState(false);

  useEffect(() => {
    if (!isEditingArgs) {
      setArgsDraft(argsToStr(server.args));
    }
  }, [isEditingArgs, server.args]);

  const toggleEnvValueVisibility = (rowId: string) => {
    setVisibleEnvValues((prev) => ({ ...prev, [rowId]: !prev[rowId] }));
  };

  const updateArgsDraft = (nextDraft: string) => {
    setIsEditingArgs(true);
    setArgsDraft(nextDraft);
    onChange({ ...server, args: strToArgs(nextDraft) });
  };

  const commitArgsDraft = () => {
    const parsedArgs = strToArgs(argsDraft);
    setIsEditingArgs(false);
    setArgsDraft(argsToStr(parsedArgs));
    if (!sameArgs(server.args, parsedArgs)) {
      onChange({ ...server, args: parsedArgs });
    }
  };

  // 添加新的环境变量 KV 对
  const addEnvVar = () => {
    onChange({ ...server, env: { ...server.env, "": "" } });
  };

  // 更新环境变量
  const updateEnvVar = (oldKey: string, newKey: string, newValue: string) => {
    const newEnv: Record<string, string> = {};
    Object.entries(server.env).forEach(([k, v]) => {
      if (k === oldKey) {
        if (newKey.trim()) {
          newEnv[newKey] = newValue;
        }
      } else {
        newEnv[k] = v;
      }
    });
    // 如果是新添加的空键
    if (oldKey === "" && newKey.trim()) {
      newEnv[newKey] = newValue;
    } else if (oldKey === "" && !newKey.trim()) {
      newEnv[""] = newValue;
    }
    onChange({ ...server, env: newEnv });
  };

  // 删除环境变量
  const removeEnvVar = (key: string) => {
    const newEnv = { ...server.env };
    delete newEnv[key];
    onChange({ ...server, env: newEnv });
  };

  return (
    <div className="server-row-wrap">
      {/* ── 服务器基本信息行 ── */}
      <div className={`server-row ${!server.enabled ? "server-row-disabled" : ""}`}>
        {/* ── 修复后的纯 CSS 滑动开关，无文字内容 ── */}
        <button
          className={`toggle-btn ${server.enabled ? "toggle-on" : "toggle-off"}`}
          title={server.enabled ? t("enabledClick") : t("disabledClick")}
          onClick={() => onChange({ ...server, enabled: !server.enabled })}
          aria-label={server.enabled ? t("enabledClick") : t("disabledClick")}
        />
        <div className="server-row-fields">
          <input className="form-input" placeholder={t("name")}
            value={server.name}
            onChange={(e) => onChange({ ...server, name: e.target.value })} />
          <input className="form-input" placeholder="npx"
            value={server.command}
            onChange={(e) => onChange({ ...server, command: e.target.value })} />
          <input className="form-input" placeholder="-y @modelcontextprotocol/server-filesystem /path"
            value={argsDraft}
            onFocus={() => setIsEditingArgs(true)}
            onChange={(e) => updateArgsDraft(e.target.value)}
            onBlur={commitArgsDraft}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.currentTarget.blur();
              }
            }} />
        </div>
        <span className={`server-test-chip ${testState.status}`} title={statusTitle}>{statusText}</span>
        <span className={`server-test-chip ${authStateTone(authState)}`} title={authTitle}>{authText}</span>
        <button
          className="btn btn-secondary btn-sm btn-test-server"
          title={t("testServerHint")}
          disabled={isTesting}
          onClick={onTest}
        >
          {isTesting ? t("serverTestTesting") : t("testServer")}
        </button>
        {showAuthActions && (
          <>
            <button className="btn btn-secondary btn-sm btn-auth-action" title={t("reauthorizeServer")} onClick={onReauthorize}>
              {t("reauthorizeServer")}
            </button>
            <button className="btn btn-secondary btn-sm btn-auth-action" title={t("clearServerAuth")} onClick={onClearAuth}>
              {t("clearServerAuth")}
            </button>
          </>
        )}
        {/* ── 添加环境变量的加号按钮 ── */}
        <button className="btn-icon btn-add-env" title={t("addEnvVar")} onClick={addEnvVar}>+</button>
        <button className="btn-icon btn-danger-icon" title={t("remove")} onClick={onDelete}>✕</button>
      </div>

      {/* ── 环境变量 KV 对列表（仅当有环境变量时显示）── */}
      {envEntries.length > 0 && (
        <div className={`server-env-row ${!server.enabled ? "server-row-disabled" : ""}`}>
          <span className="env-label">{t("envVars")}</span>
          <div className="env-kv-list">
            {envEntries.map(([key, value], idx) => {
              const rowId = `${idx}:${key}`;
              const isVisible = visibleEnvValues[rowId] === true;
              return (
                <div className="env-kv-item" key={idx}>
                  <input
                    className="form-input env-key-input"
                    placeholder="KEY"
                    value={key}
                    onChange={(e) => updateEnvVar(key, e.target.value, value)}
                  />
                  <span className="env-kv-sep">=</span>
                  <div className="env-value-wrap">
                    <input
                      className="form-input env-value-input"
                      type={isVisible ? "text" : "password"}
                      autoComplete="off"
                      placeholder="VALUE"
                      value={value}
                      onChange={(e) => updateEnvVar(key, key, e.target.value)}
                    />
                    <button
                      type="button"
                      className="btn-icon btn-env-visibility"
                      title={isVisible ? t("hideEnvValue") : t("showEnvValue")}
                      aria-label={isVisible ? t("hideEnvValue") : t("showEnvValue")}
                      onClick={() => toggleEnvValueVisibility(rowId)}
                    >
                      {isVisible ? <EyeOff size={12} /> : <Eye size={12} />}
                    </button>
                  </div>
                  <button className="btn-icon btn-danger-icon btn-remove-env" title={t("removeEnvVar")} onClick={() => removeEnvVar(key)}>✕</button>
                </div>
              );
            })}
          </div>
        </div>
      )}

      {/* ── 运行时端点链接（直接放在 server-row 内部底部）── */}
      {showLinks && (
        <div className={`server-row-endpoints ${!server.enabled ? "server-row-disabled" : ""}`}>
          <div className="endpoint-item">
            <span className="endpoint-label">{t("endpointSSE")}</span>
            <code className="endpoint-url">{sseUrl}</code>
            <button className="btn-icon" title={t("copySSE")}
              onClick={() => onCopy(server.name, "sse", sseUrl, `${server.name}-sse`)}>
              {copied === `${server.name}-sse`
                ? <Check size={12} color="var(--accent-green)" />
                : <Copy size={12} />}
            </button>
          </div>
          <div className="endpoint-item">
            <span className="endpoint-label">{t("endpointHTTP")}</span>
            <code className="endpoint-url">{httpUrl}</code>
            <button className="btn-icon" title={t("copyHTTP")}
              onClick={() => onCopy(server.name, "streamable-http", httpUrl, `${server.name}-http`)}>
              {copied === `${server.name}-http`
                ? <Check size={12} color="var(--accent-green)" />
                : <Copy size={12} />}
            </button>
          </div>
        </div>
      )}
    </div>
  );
}

