import { useState } from "react";
import { X, ChevronDown, ChevronUp } from "lucide-react";
import type { TFunction } from "../i18n";
import type { AiToolDef } from "../types";

interface SessionToolsModalProps {
  open: boolean;
  sessionName: string;
  tools: AiToolDef[];
  enabledTools: Set<string>;
  onToggle: (toolName: string) => void;
  onClose: () => void;
  t: TFunction;
}

function ToolDescription({ description, maxLines = 2 }: { description: string; maxLines?: number }) {
  const [expanded, setExpanded] = useState(false);
  const lines = description.split("\n");
  const needsTruncation = lines.length > maxLines;

  return (
    <div className="stm-tool-desc-wrap">
      <div className={`stm-tool-desc ${!expanded && needsTruncation ? "stm-tool-desc-truncated" : ""}`}>
        {description}
      </div>
      {needsTruncation && (
        <button
          className="stm-tool-desc-expand"
          onClick={() => setExpanded(!expanded)}
          title={expanded ? "收起" : "展开完整描述"}
        >
          {expanded ? <ChevronUp size={12} /> : <ChevronDown size={12} />}
          {expanded ? "收起" : "展开"}
        </button>
      )}
    </div>
  );
}

export function SessionToolsModal({ open, sessionName, tools, enabledTools, onToggle, onClose, t }: SessionToolsModalProps) {
  if (!open) return null;

  return (
    <div className="modal-overlay" onClick={onClose}>
      <div className="modal-content stm-modal" onClick={(e) => e.stopPropagation()}>
        <div className="modal-header stm-modal-header">
          <div className="stm-modal-title-group">
            <span className="stm-modal-title">{t("sessionToolsModalTitle")}</span>
            <span className="stm-modal-subtitle">{sessionName}</span>
          </div>
          <button className="btn-icon stm-modal-close" onClick={onClose} title={t("planViewClose")}>
            <X size={16} />
          </button>
        </div>
        <div className="modal-body stm-modal-body">
          {tools.length === 0 ? (
            <div className="stm-empty">{t("sessionToolsEmpty")}</div>
          ) : (
            <div className="stm-tool-list">
              {tools.map((tool) => {
                const isOn = enabledTools.has(tool.name);
                return (
                  <div className="stm-tool-row" key={tool.name}>
                    <button
                      className={`toggle-btn ${isOn ? "toggle-on" : "toggle-off"}`}
                      onClick={() => onToggle(tool.name)}
                      title={isOn ? t("enabledClick") : t("disabledClick")}
                      aria-label={`${isOn ? t("enabledClick") : t("disabledClick")} ${tool.name}`}
                      aria-pressed={isOn}
                    />
                    <div className="stm-tool-info">
                      <span className="stm-tool-name">{tool.name}</span>
                      <ToolDescription description={tool.description || ""} />
                    </div>
                  </div>
                );
              })}
            </div>
          )}
        </div>
        <div className="modal-footer stm-modal-footer">
          <span className="stm-modal-footer-hint">{t("sessionToolsFooterHint")}</span>
          <button className="btn btn-secondary btn-sm" onClick={onClose}>{t("planViewClose")}</button>
        </div>
      </div>
    </div>
  );
}
