import { Plus, Settings as Gear } from "lucide-react";
import { useI18n } from "../i18n";
import { useStore, type Tab } from "../lib/store";
import { Droplet, IconButton, Segmented } from "./ui";

export function Header({ onSettings, onLink }: { onSettings: () => void; onLink: () => void }) {
  const { state, dispatch } = useStore();
  const { t } = useI18n();
  // macOS: the window uses an overlay title bar, so leave room for the
  // traffic lights and make the header the drag region.
  const mac = state.platform === "macos";
  return (
    <header
      data-tauri-drag-region
      className={`flex h-[52px] shrink-0 items-center gap-3 pr-3 ${mac ? "pl-[92px]" : "pl-5"}`}
    >
      <div data-tauri-drag-region className="flex min-w-0 flex-1 items-center gap-2">
        <span className="text-accent" aria-hidden>
          <Droplet size={18} />
        </span>
        <span data-tauri-drag-region className="truncate text-[15px] font-semibold tracking-[-0.01em]">
          {t("app_name")}
        </span>
      </div>
      <Segmented<Tab>
        label={t("app_name")}
        value={state.tab}
        onChange={(tab) => dispatch({ type: "tab", tab })}
        options={[
          { value: "my", label: t("tab_my_devices") },
          { value: "nearby", label: t("tab_nearby") },
          { value: "code", label: t("tab_quick_code") },
        ]}
      />
      <div data-tauri-drag-region className="flex flex-1 items-center justify-end gap-1">
        {state.tab === "my" && (
          <IconButton label={t("link_device")} onClick={onLink}>
            <Plus size={17} strokeWidth={1.75} />
          </IconButton>
        )}
        <IconButton label={t("settings")} onClick={onSettings}>
          <Gear size={17} strokeWidth={1.75} />
        </IconButton>
      </div>
    </header>
  );
}
