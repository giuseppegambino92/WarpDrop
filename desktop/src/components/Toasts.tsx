import { CheckCircle2, CircleAlert, Info } from "lucide-react";
import { useStore } from "../lib/store";

export function Toasts() {
  const { state, dispatch } = useStore();
  return (
    <div className="pointer-events-none fixed top-[60px] left-1/2 z-50 flex w-[min(440px,calc(100%-32px))] -translate-x-1/2 flex-col items-center gap-2" aria-live="polite">
      {state.toasts.map((toast) => (
        <button
          key={toast.id}
          type="button"
          onClick={() => dispatch({ type: "dropToast", id: toast.id })}
          className="sheet-in pointer-events-auto flex max-w-full items-start gap-2 rounded-control bg-surface px-3 py-2 text-left text-[13px] leading-[18px]"
          style={{ boxShadow: "inset 0 0 0 1px var(--border), 0 6px 24px rgba(0,0,0,0.14)" }}
        >
          <span className="mt-px shrink-0">
            {toast.kind === "success" ? (
              <CheckCircle2 size={15} className="text-success" />
            ) : toast.kind === "error" ? (
              <CircleAlert size={15} className="text-danger" />
            ) : (
              <Info size={15} className="text-accent" />
            )}
          </span>
          <span>{toast.text}</span>
        </button>
      ))}
    </div>
  );
}
