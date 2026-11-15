// Device cards for "My Devices" and "Nearby", plus their empty states.
import { Link2 } from "lucide-react";
import { useI18n } from "../i18n";
import { useActions } from "../lib/actions";
import { myDevices, nearbyDevices, useStore, type DeviceModel } from "../lib/store";
import { Badge, Button, DeviceGlyph, PathBadge } from "./ui";

export interface DragState {
  active: boolean;
  /** Device under the cursor while dragging. */
  hoverId: string | null;
}

function DeviceCard({ device, drag, selected, fresh }: { device: DeviceModel; drag: DragState; selected: boolean; fresh: boolean }) {
  const { t } = useI18n();
  const actions = useActions();
  const offline = device.trusted && !device.online;
  const hovered = drag.active && drag.hoverId === device.deviceId;
  const highlighted = drag.active || selected;

  const scale = hovered ? 1.04 : drag.active ? 1.02 : 1;
  const ring = drag.active
    ? "inset 0 0 0 1.5px var(--accent)"
    : selected
      ? "inset 0 0 0 1px color-mix(in srgb, var(--accent) 40%, transparent)"
      : "inset 0 0 0 1px var(--border)";

  return (
    <button
      type="button"
      data-peer-id={device.deviceId}
      onClick={() => void actions.activateDevice(device.deviceId)}
      onContextMenu={(e) => {
        e.preventDefault();
        void actions.showDeviceMenu(device);
      }}
      className={`device-card group flex min-w-0 items-center gap-3 rounded-card p-3 text-left ${fresh ? "spring-in" : ""} ${
        highlighted ? "bg-accent-soft" : "bg-surface hover:bg-surface-hover"
      } ${offline && !drag.active ? "opacity-50" : ""}`}
      style={{ transform: `scale(${scale})`, boxShadow: ring }}
    >
      <span
        className={`flex h-12 w-12 shrink-0 items-center justify-center rounded-[10px] text-text-secondary transition-colors duration-150 ${
          highlighted ? "bg-surface" : "bg-surface-hover group-hover:bg-surface"
        }`}
      >
        <DeviceGlyph kind={device.kind} />
      </span>
      <span className="flex min-w-0 flex-1 flex-col gap-1">
        <span className="truncate text-[14px] leading-5 font-semibold">{device.name}</span>
        <span className="flex min-h-5 flex-wrap items-center gap-1">
          {offline ? <span className="type-caption text-text-tertiary">{t("not_nearby")}</span> : <PathBadge path={device.bestPath} />}
          {device.trusted && <Badge tone="success">{t("badge_trusted")}</Badge>}
        </span>
      </span>
    </button>
  );
}

function Searching({ text }: { text: string }) {
  const { t } = useI18n();
  return (
    <div className="flex flex-col items-center gap-3 px-6 py-12 text-center">
      <span className="flex items-center gap-2 type-section text-text-secondary">
        <span className="searching-dot h-2 w-2 rounded-full bg-accent" aria-hidden />
        {t("searching")}
      </span>
      <p className="max-w-sm type-body text-text-tertiary">{text}</p>
    </div>
  );
}

export function DeviceGrid({ drag, onLink }: { drag: DragState; onLink: () => void }) {
  const { state } = useStore();
  const { t } = useI18n();
  const devices = state.tab === "my" ? myDevices(state) : nearbyDevices(state);

  if (devices.length === 0) {
    if (state.tab === "nearby") return <Searching text={t("no_nearby")} />;
    return (
      <div className="flex flex-col items-center gap-4 px-6 py-12 text-center">
        <p className="max-w-sm type-body text-text-secondary">{t("no_my_devices")}</p>
        <Button variant="secondary" onClick={onLink}>
          <Link2 size={15} strokeWidth={1.75} />
          {t("link_device")}
        </Button>
      </div>
    );
  }

  return (
    <div className="grid gap-3 [grid-template-columns:repeat(auto-fill,minmax(210px,1fr))]">
      {devices.map((d) => (
        <DeviceCard
          key={d.deviceId}
          device={d}
          drag={drag}
          selected={state.selectedPeerId === d.deviceId}
          fresh={state.freshlyLinked.includes(d.deviceId)}
        />
      ))}
    </div>
  );
}
