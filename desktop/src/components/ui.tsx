// Small shared building blocks.
import { forwardRef, useEffect, useRef, type ButtonHTMLAttributes, type ReactNode, type SVGProps } from "react";
import { Laptop, Monitor, MonitorSmartphone, Smartphone, Tablet, X, type LucideProps } from "lucide-react";
import type { DeviceKind, PathKind } from "../lib/types";
import { useI18n, type StringKey } from "../i18n";

type ButtonVariant = "primary" | "secondary" | "ghost" | "danger";

const variants: Record<ButtonVariant, string> = {
  primary: "bg-accent text-white hover:brightness-110 active:brightness-95 disabled:opacity-40",
  secondary: "bg-surface text-text hairline hover:bg-surface-hover active:bg-surface-hover disabled:opacity-40",
  ghost: "text-text-secondary hover:bg-surface-hover hover:text-text disabled:opacity-40",
  danger: "text-danger hover:bg-surface-hover disabled:opacity-40",
};

export const Button = forwardRef<
  HTMLButtonElement,
  ButtonHTMLAttributes<HTMLButtonElement> & { variant?: ButtonVariant; size?: "sm" | "md" }
>(function Button({ variant = "secondary", size = "md", className = "", ...rest }, ref) {
  const sz = size === "sm" ? "h-7 px-2.5 type-caption" : "h-8 px-3 text-[13px] font-medium";
  return (
    <button
      ref={ref}
      type="button"
      className={`inline-flex shrink-0 items-center justify-center gap-1.5 rounded-control whitespace-nowrap transition-colors duration-150 ${sz} ${variants[variant]} ${className}`}
      {...rest}
    />
  );
});

/** Square icon-only button with an accessible label + native tooltip. */
export function IconButton({
  label,
  children,
  className = "",
  ...rest
}: ButtonHTMLAttributes<HTMLButtonElement> & { label: string; children: ReactNode }) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      className={`inline-flex h-8 w-8 shrink-0 items-center justify-center rounded-control text-text-secondary transition-colors duration-150 hover:bg-surface-hover hover:text-text disabled:opacity-40 ${className}`}
      {...rest}
    >
      {children}
    </button>
  );
}

export function Badge({ tone = "neutral", children }: { tone?: "neutral" | "cable" | "success" | "accent"; children: ReactNode }) {
  const tones = {
    neutral: "bg-surface-hover text-text-secondary",
    cable: "bg-[color-mix(in_srgb,var(--cable)_14%,transparent)] text-cable",
    success: "bg-[color-mix(in_srgb,var(--success)_14%,transparent)] text-success",
    accent: "bg-accent-soft text-accent",
  } as const;
  return <span className={`inline-flex h-5 items-center rounded-badge px-1.5 type-caption whitespace-nowrap ${tones[tone]}`}>{children}</span>;
}

const pathKeys: Partial<Record<PathKind, StringKey>> = {
  UsbCable: "badge_usb",
  AdbUsb: "badge_usb",
  ThunderboltBridge: "badge_thunderbolt",
  WifiDirect: "badge_wifi_direct",
  Ethernet: "badge_ethernet",
  Wifi: "badge_wifi",
  Lan: "badge_lan",
  Loopback: "this_computer",
  InternetP2p: "badge_p2p",
  Relay: "badge_relay",
};

export function PathBadge({ path }: { path: PathKind | null }) {
  const { t } = useI18n();
  if (!path) return null;
  const key = pathKeys[path];
  if (!key) return null;
  const wired = path === "UsbCable" || path === "AdbUsb" || path === "ThunderboltBridge";
  // `key` re-mounts on failover so the change is visible (fade).
  return (
    <span key={path} className="fade-in inline-flex">
      <Badge tone={wired ? "cable" : "neutral"}>{t(key)}</Badge>
    </span>
  );
}

/** Android head glyph in Lucide's style (Lucide ships no brand icons). */
function AndroidGlyph({ size = 24, strokeWidth = 2, ...rest }: LucideProps) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={strokeWidth}
      strokeLinecap="round"
      strokeLinejoin="round"
      {...(rest as SVGProps<SVGSVGElement>)}
    >
      <path d="M4 17a8 8 0 0 1 16 0v1H4z" />
      <path d="m7 9.5-2-3" />
      <path d="m17 9.5 2-3" />
      <circle cx="9" cy="14" r="0.6" fill="currentColor" />
      <circle cx="15" cy="14" r="0.6" fill="currentColor" />
    </svg>
  );
}

export function DeviceGlyph({ kind, size = 40, strokeWidth = 1.25 }: { kind: DeviceKind; size?: number; strokeWidth?: number }) {
  const p = { size, strokeWidth, "aria-hidden": true } as const;
  switch (kind) {
    case "Mac":
      return <Laptop {...p} />;
    case "IPhone":
      return <Smartphone {...p} />;
    case "IPad":
      return <Tablet {...p} />;
    case "Android":
      return <AndroidGlyph {...p} />;
    case "Windows":
    case "Linux":
      return <Monitor {...p} />;
    default:
      return <MonitorSmartphone {...p} />;
  }
}

/** The WarpDrop droplet (same geometry as the tray and app icon). */
export function Droplet({ size = 16 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" aria-hidden="true">
      <path d="M12 1.8L17.81 11.15A6.84 6.84 0 1 1 6.19 11.15Z" fill="currentColor" />
    </svg>
  );
}

/** Modal sheet with scrim, Escape to close and focus containment on open. */
export function Sheet({
  title,
  onClose,
  children,
  width = 420,
  dismissible = true,
}: {
  title?: string;
  onClose: () => void;
  children: ReactNode;
  width?: number;
  dismissible?: boolean;
}) {
  const { t } = useI18n();
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const prev = document.activeElement as HTMLElement | null;
    ref.current?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && dismissible) {
        e.stopPropagation();
        onClose();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("keydown", onKey);
      prev?.focus?.();
    };
  }, [onClose, dismissible]);
  return (
    <div
      className="fade-in fixed inset-0 z-40 flex items-center justify-center bg-[var(--scrim)] p-4"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget && dismissible) onClose();
      }}
    >
      <div
        ref={ref}
        role="dialog"
        aria-modal="true"
        aria-label={title}
        tabIndex={-1}
        className="sheet-in relative max-h-full w-full overflow-auto rounded-card bg-surface p-5 hairline outline-none"
        style={{ maxWidth: width, boxShadow: "inset 0 0 0 1px var(--border), 0 12px 40px rgba(0,0,0,0.18)" }}
      >
        {title !== undefined && (
          <div className="mb-4 flex items-center justify-between gap-3">
            <h2 className="type-title">{title}</h2>
            {dismissible && (
              <IconButton label={t("close")} onClick={onClose}>
                <X size={16} />
              </IconButton>
            )}
          </div>
        )}
        {children}
      </div>
    </div>
  );
}

export function Segmented<T extends string>({
  value,
  options,
  onChange,
  label,
  size = "md",
}: {
  value: T;
  options: { value: T; label: string }[];
  onChange: (v: T) => void;
  label: string;
  size?: "sm" | "md";
}) {
  return (
    <div role="tablist" aria-label={label} className="inline-flex shrink-0 rounded-control bg-surface-hover p-0.5 hairline">
      {options.map((o) => {
        const active = o.value === value;
        return (
          <button
            key={o.value}
            type="button"
            role="tab"
            aria-selected={active}
            onClick={() => onChange(o.value)}
            className={`rounded-[6px] whitespace-nowrap transition-colors duration-150 ${size === "sm" ? "h-6 px-2.5 type-caption" : "h-7 px-3 text-[13px] font-medium"} ${
              active ? "bg-surface text-text hairline" : "text-text-secondary hover:text-text"
            }`}
          >
            {o.label}
          </button>
        );
      })}
    </div>
  );
}

export function Switch({ checked, onChange, label }: { checked: boolean; onChange: (v: boolean) => void; label: string }) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      onClick={() => onChange(!checked)}
      className={`relative h-5 w-9 shrink-0 rounded-full transition-colors duration-200 ${checked ? "bg-accent" : "bg-[color-mix(in_srgb,var(--text-tertiary)_45%,transparent)]"}`}
    >
      <span
        className="absolute top-0.5 left-0.5 h-4 w-4 rounded-full bg-white shadow-sm transition-transform duration-200 ease-spring"
        style={{ transform: checked ? "translateX(16px)" : "none" }}
      />
    </button>
  );
}
