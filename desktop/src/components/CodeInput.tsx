// 6-digit code field: auto-inserts the dash, accepts pasted codes and
// `warpdrop://` links, and reports completion once per complete entry.
import { forwardRef, type ClipboardEvent, type KeyboardEvent } from "react";
import { formatCodeDigits, parseCodeText, type CodeEntry } from "../lib/format";

interface Props {
  value: CodeEntry;
  onChange: (v: CodeEntry) => void;
  /** Fired when the entry becomes complete (typed 6th digit, paste, or Enter). */
  onComplete: (v: CodeEntry) => void;
  label: string;
  large?: boolean;
  disabled?: boolean;
  autoFocus?: boolean;
}

export const CodeInput = forwardRef<HTMLInputElement, Props>(function CodeInput(
  { value, onChange, onComplete, label, large = false, disabled, autoFocus },
  ref,
) {
  const apply = (next: CodeEntry) => {
    const wasComplete = value.digits.length === 6;
    onChange(next);
    if (next.digits.length === 6 && (!wasComplete || next.uri !== value.uri)) onComplete(next);
  };

  const onInput = (raw: string) => {
    const parsed = parseCodeText(raw);
    if (parsed?.uri) return apply(parsed);
    const digits = raw.replace(/\D/g, "").slice(0, 6);
    // Editing digits invalidates a pasted link.
    apply({ digits, uri: digits === value.digits ? value.uri : null });
  };

  const onPaste = (e: ClipboardEvent<HTMLInputElement>) => {
    const text = e.clipboardData.getData("text");
    const parsed = parseCodeText(text);
    if (parsed && (parsed.uri || parsed.digits.length === 6)) {
      e.preventDefault();
      apply(parsed);
    }
  };

  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Enter" && value.digits.length === 6) {
      e.preventDefault();
      onComplete(value);
    }
  };

  return (
    <input
      ref={ref}
      aria-label={label}
      inputMode="numeric"
      autoComplete="off"
      autoCorrect="off"
      spellCheck={false}
      autoFocus={autoFocus}
      disabled={disabled}
      placeholder="000-000"
      value={formatCodeDigits(value.digits)}
      onChange={(e) => onInput(e.target.value)}
      onPaste={onPaste}
      onKeyDown={onKeyDown}
      className={`rounded-control bg-surface text-text tabular hairline outline-none placeholder:text-text-tertiary focus:shadow-[inset_0_0_0_1.5px_var(--accent)] disabled:opacity-50 ${
        large ? "h-14 w-56 text-center type-code" : "h-8 w-[104px] px-2.5 text-center text-[15px] font-semibold tracking-[0.04em]"
      }`}
    />
  );
});
