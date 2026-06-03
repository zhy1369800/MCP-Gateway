import { useState, useEffect } from "react";
import { X } from "lucide-react";
import type { TFunction } from "../i18n";

interface SystemPromptModalProps {
  open: boolean;
  initialText: string;
  onSave: (text: string) => void;
  onClose: () => void;
  t: TFunction;
}

export function SystemPromptModal({ open, initialText, onSave, onClose, t }: SystemPromptModalProps) {
  const [text, setText] = useState(initialText);

  useEffect(() => {
    if (open) {
      setText(initialText);
    }
  }, [open, initialText]);

  if (!open) return null;

  return (
    <div className="modal-overlay" onClick={onClose}>
      <div className="modal-content stm-modal" onClick={(e) => e.stopPropagation()}>
        <div className="modal-header stm-modal-header">
          <div className="stm-modal-title-group">
            <span className="stm-modal-title">{t("systemPromptModalTitle")}</span>
          </div>
          <button className="btn-icon stm-modal-close" onClick={onClose} title={t("planViewClose")}>
            <X size={16} />
          </button>
        </div>
        <div className="modal-body stm-modal-body" style={{ padding: "20px" }}>
          <textarea
            className="form-textarea"
            value={text}
            onChange={(e) => setText(e.target.value)}
            placeholder={t("systemPromptPlaceholder")}
            style={{ minHeight: "300px", fontSize: "13px" }}
          />
        </div>
        <div className="modal-footer stm-modal-footer">
          <button className="btn btn-secondary btn-sm" onClick={onClose}>{t("planViewClose")}</button>
          <button className="btn btn-start btn-sm" onClick={() => onSave(text)}>
            {t("systemPromptSave")}
          </button>
        </div>
      </div>
    </div>
  );
}
