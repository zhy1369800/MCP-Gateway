import { useRef } from "react";

// ── JsonEditor 组件（无高亮）───────────────────────
interface Props {
  value: string;
  onChange: (v: string) => void;
  placeholder?: string;
}

export default function JsonEditor({ value, onChange, placeholder }: Props) {
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  return (
    <div className="json-editor-container">
      <textarea
        ref={textareaRef}
        className="json-textarea"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        spellCheck={false}
        placeholder={placeholder}
        autoComplete="off"
        autoCorrect="off"
        autoCapitalize="off"
      />
    </div>
  );
}

