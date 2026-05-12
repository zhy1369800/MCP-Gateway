import { useEffect, useState } from "react";
import { ChevronDown, ChevronRight } from "lucide-react";
import type { Lang, TFunction } from "../i18n";
import type { SkillConfirmation } from "../types";
import { parseSkillReason } from "../features/skills/defaultRuleReasons";
import { formatTime } from "../utils/display";

function skillPreviewLabel(kind: string | undefined, t: TFunction): string {
  if (kind === "edit") return t("editPreview");
  return t("commandPreview");
}

export function SkillConfirmations({ pending, busyIds, onApprove, onReject, lang, t }: {
  pending: SkillConfirmation[];
  busyIds: Set<string>;
  onApprove: (id: string) => void;
  onReject: (id: string) => void;
  lang: Lang;
  t: TFunction;
}) {
  if (pending.length === 0) {
    return <div className="empty-hint">{t("noSkillPending")}</div>;
  }

  return (
    <div className="skill-confirm-list">
      {pending.map((item) => {
        const busy = busyIds.has(item.id);
        const displayName = item.displayName.trim();
        const showDisplayName = displayName.length > 0 && displayName !== item.skill;
        const reason = parseSkillReason(item.reason, lang, item.reasonKey);
        const operationText = item.preview || item.rawCommand;
        return (
          <div className="skill-confirm-item" key={item.id}>
            <div className="skill-confirm-head">
              <div className="skill-confirm-meta">
                <span className="skill-chip">{item.skill}</span>
                {showDisplayName && <span className="skill-script">{displayName}</span>}
              </div>
              <div className="skill-confirm-actions">
                <button className="btn btn-secondary btn-sm skill-confirm-reject-btn" disabled={busy} onClick={() => onReject(item.id)}>
                  {t("reject")}
                </button>
                <button className="btn btn-start btn-sm" disabled={busy} onClick={() => onApprove(item.id)}>
                  {t("approve")}
                </button>
              </div>
            </div>
            <div className="skill-confirm-list-summary">
              <div className="skill-confirm-summary-row">
                <span className="skill-confirm-summary-label">{skillPreviewLabel(item.kind, t)}</span>
                <code className="skill-command skill-confirm-command-main">{operationText}</code>
              </div>
              <div className="skill-confirm-summary-row">
                <span className="skill-confirm-summary-label">{t("confirmReason")}</span>
                <span className="skill-confirm-purpose">{reason.text}</span>
              </div>
              {item.cwd && (
                <div className="skill-confirm-summary-row">
                  <span className="skill-confirm-summary-label">{t("cwd")}</span>
                  <code className="skill-command">{item.cwd}</code>
                </div>
              )}
              {item.affectedPaths && item.affectedPaths.length > 0 && (
                <div className="skill-confirm-summary-row">
                  <span className="skill-confirm-summary-label">{t("affectedPaths")}</span>
                  <code className="skill-command">{item.affectedPaths.join("\n")}</code>
                </div>
              )}
              <div className="skill-confirm-summary-row">
                <span className="skill-confirm-summary-label">{t("createdAt")}</span>
                <span className="skill-confirm-muted">{formatTime(item.createdAt)}</span>
              </div>
              <div className="skill-confirm-list-note">{t("skillsConfirmOneTimeHint")}</div>
            </div>
          </div>
        );
      })}
    </div>
  );
}

export function SkillConfirmationPopup({
  open,
  item,
  busy,
  onApprove,
  onReject,
  onLater,
  lang,
  t,
}: {
  open: boolean;
  item: SkillConfirmation | null;
  busy: boolean;
  onApprove: (id: string) => void;
  onReject: (id: string) => void;
  onLater: (id: string) => void;
  lang: Lang;
  t: TFunction;
}) {
  const [detailsOpen, setDetailsOpen] = useState(false);

  useEffect(() => {
    setDetailsOpen(false);
  }, [item?.id]);

  if (!open || !item) return null;
  const displayName = item.displayName.trim();
  const showDisplayName = displayName.length > 0 && displayName !== item.skill;
  const reason = parseSkillReason(item.reason, lang, item.reasonKey);
  const operationText = item.preview || item.rawCommand;
  return (
    <div className="modal-overlay" onClick={() => onLater(item.id)}>
      <div className="modal-content skill-confirm-popup" onClick={(e) => e.stopPropagation()}>
        <div className="modal-header skill-confirm-popup-header">
          <div className="modal-title">{t("skillsConfirmPopupTitle")}</div>
        </div>
        <div className="modal-body skill-confirm-popup-body">
          <div className="skill-confirm-lead">
            <div className="skill-confirm-lead-title">{t("skillsConfirmPopupMsg")}</div>
            <div className="skill-confirm-timeout">{t("skillsConfirmTimeoutHint")}</div>
            <div className="skill-confirm-one-time">{t("skillsConfirmOneTimeHint")}</div>
          </div>

          <div className="skill-confirm-summary">
            <div className="skill-confirm-summary-row">
              <span className="skill-confirm-summary-label">{t("confirmOperationLabel")}</span>
              <code className="skill-command skill-confirm-command-main">{operationText}</code>
            </div>
            <div className="skill-confirm-summary-row">
              <span className="skill-confirm-summary-label">{t("confirmReason")}</span>
              <span className="skill-confirm-purpose">{reason.text}</span>
            </div>
            {item.cwd && (
              <div className="skill-confirm-summary-row">
                <span className="skill-confirm-summary-label">{t("cwd")}</span>
                <code className="skill-command">{item.cwd}</code>
              </div>
            )}
            {item.affectedPaths && item.affectedPaths.length > 0 && (
              <div className="skill-confirm-summary-row">
                <span className="skill-confirm-summary-label">{t("affectedPaths")}</span>
                <code className="skill-command">{item.affectedPaths.join("\n")}</code>
              </div>
            )}
          </div>

          <button className="skill-confirm-details-toggle" type="button" onClick={() => setDetailsOpen((open) => !open)}>
            {detailsOpen ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
            {detailsOpen ? t("confirmHideTechnicalDetails") : t("confirmTechnicalDetails")}
          </button>

          {detailsOpen && (
            <div className="skill-confirm-technical">
              <div className="skill-confirm-meta">
                <span className="skill-chip">{item.skill}</span>
                {showDisplayName && <span className="skill-script">{displayName}</span>}
              </div>
              <div className="skill-confirm-tech-grid">
                <span className="field-label">{skillPreviewLabel(item.kind, t)}</span>
                <code className="skill-command">{operationText}</code>
                <span className="field-label">{t("confirmRuleLabel")}</span>
                <span>{reason.ruleId || "-"}</span>
                <span className="field-label">{t("confirmReasonKeyLabel")}</span>
                <span>{reason.reasonKey || "-"}</span>
                <span className="field-label">{t("confirmSourceLabel")}</span>
                <span>{reason.source || "-"}</span>
                <span className="field-label">{t("createdAt")}</span>
                <span>{formatTime(item.createdAt)}</span>
                <span className="field-label">{t("confirmRawReasonLabel")}</span>
                <span>{reason.raw}</span>
              </div>
            </div>
          )}
        </div>
        <div className="modal-footer">
          <button className="btn btn-secondary" disabled={busy} onClick={() => onLater(item.id)}>
            {t("decideLater")}
          </button>
          <button className="btn btn-secondary skill-confirm-reject-btn" disabled={busy} onClick={() => onReject(item.id)}>
            {t("reject")}
          </button>
          <button className="btn btn-start" disabled={busy} onClick={() => onApprove(item.id)}>
            {t("approve")}
          </button>
        </div>
      </div>
    </div>
  );
}

