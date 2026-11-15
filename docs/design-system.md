# WarpDrop design system

One product, five platforms. Each platform uses its **native** controls and
materials; this document fixes what must be identical everywhere: layout,
hierarchy, tokens, copy, motion.

## Principles
1. **One screen.** Everything important is on the main window: devices, your
   code, the send-to-code field, live transfers. No settings spelunking.
2. **Content over chrome.** No gradients, no neon, no glow, no glassmorphism
   stacks, no decorative charts. One accent color, used sparingly.
3. **Honest telemetry.** Progress, speed and ETA come straight from the engine.
   Never animate fake progress. Indeterminate states say what is happening
   ("Connecting…", "Waiting for Pixel to accept…").
4. **Quiet confidence.** Success = a check mark and the file name, then it gets
   out of the way. Errors say what happened and what to do.

## Tokens

| Token | Light | Dark |
|---|---|---|
| `bg` (window) | `#F6F6F7` (or native material) | `#141416` (or native material) |
| `surface` (cards) | `#FFFFFF` | `#1C1C1F` |
| `surface-hover` | `#F1F1F3` | `#232327` |
| `border` (1 px, inner) | `rgba(0,0,0,0.08)` | `rgba(255,255,255,0.08)` |
| `text` | `#111113` | `#EDEDEF` |
| `text-secondary` | `#6B6B73` | `#9A9AA3` |
| `text-tertiary` | `#9A9AA3` | `#6B6B73` |
| `accent` | `#2F6BFF` | `#4C82FF` |
| `accent-soft` (selection, drop highlight) | `rgba(47,107,255,0.10)` | `rgba(76,130,255,0.16)` |
| `success` | `#1F9D55` | `#34C26F` |
| `warning` | `#C27C0E` | `#E0A030` |
| `danger` | `#D93D3D` | `#F06262` |
| `cable` (⚡ badge tint) | `#C27C0E` | `#E0A030` |

* **Radius**: cards 12, buttons/fields 8, badges 6 (pill when single word).
* **Spacing scale**: 4, 8, 12, 16, 20, 24, 32. Window padding 20 (desktop) /
  16 (phone).
* **Typography**: SF Pro (Apple), Roboto/Google Sans via Material 3 (Android),
  Inter (desktop). Sizes: title 20/600, section 13/600 uppercase-free,
  body 14/400 (desktop) 16/400 (mobile), caption 12/500, code digits
  28/600 tabular/monospaced digits with 0.04em tracking.
* **Elevation**: none on cards — a 1 px inner border separates surfaces.
  Popovers/sheets use the platform material.
* **Icons**: SF Symbols (Apple), Material Symbols Rounded (Android),
  Lucide (desktop). Device glyphs: laptop, desktop, phone, tablet, Android.

## Motion
* Spring: response 0.35 s, damping 0.8 (SwiftUI `.spring(response:0.35,
  dampingFraction:0.8)`; Compose `spring(dampingRatio=0.8f,
  stiffness=Spring.StiffnessMediumLow)`; CSS `cubic-bezier(.2,.8,.2,1)` 280 ms).
* Drag over window → device cards scale to 1.02 and gain `accent-soft` fill
  + 1.5 px accent border; the hovered card scales 1.04.
* Progress bars animate value changes over 150 ms linear (no easing bounce).
* Nothing loops forever except the "searching" dot pulse (opacity 0.4↔1, 1.2 s).

## Main window layout (desktop / macOS / iPad)

```
┌──────────────────────────────────────────────────────────────┐
│  WarpDrop            [My Devices | Nearby | Quick Code]   ⚙  │  ← segmented
├──────────────────────────────────────────────────────────────┤
│  Your code  849-201  ⧉  ▦QR        Send to code [___-___] →  │  ← code bar
├──────────────────────────────────────────────────────────────┤
│  ┌───────────────┐ ┌───────────────┐ ┌───────────────┐       │
│  │ 💻 MacBook Pro │ │ 📱 Pixel 9    │ │ 🖥 ThinkPad    │       │  ← device grid
│  │ ⚡ USB cable   │ │ Wi-Fi         │ │ LAN           │       │
│  │ ✓ My account  │ │ ✓ My account  │ │               │       │
│  └───────────────┘ └───────────────┘ └───────────────┘       │
│                                                              │
│  Drop files, folders or text anywhere                       │  ← empty-state hint
├──────────────────────────────────────────────────────────────┤
│  Holiday Photos → Pixel 9   ████████░░  82%  412 MB/s  6 s ⏸ ✕│  ← transfer HUD
└──────────────────────────────────────────────────────────────┘
```

Phone: same order, vertical — segmented control, code bar (two rows), device
list (rows, not grid), transfers as a bottom sheet / list section.

### Device card
* 40 px platform glyph in a 48 px rounded square (`surface-hover`).
* Name (body, 600), one line, ellipsis.
* Connection badge (caption) from `PeerInfo.best_path`:
  `UsbCable/AdbUsb` → "⚡ USB cable" (cable tint), `ThunderboltBridge` →
  "⚡ Thunderbolt", `WifiDirect` → "Wi-Fi Direct", `Ethernet` → "Ethernet",
  `Wifi` → "Wi-Fi", `Lan` → "LAN", `InternetP2p` → "☁️ P2P", `Relay` →
  "☁️ Relay".
* Trust badge when `trusted`: "✓ My account" (success tint).
* Offline trusted devices (in My Devices, `online == false`) are shown at 50%
  opacity with "Not nearby".
* Click → file picker; drop → send; right-click/long-press → Send clipboard,
  Remove from My Devices.

### Code bar
* Left: "Your code" + `CodeInfo.display` in the code style, copy button, QR
  button (popover with the QR from `qr_matrix(code.qr_payload)` drawn as
  crisp squares, 4-module quiet zone, plus "Expires in 7:42").
* Right: "Send to code" — a 6-digit field that auto-inserts the dash, accepts
  paste of `482910`, `482-910` or a `warpdrop://` link. When 6 digits are
  present: if files are staged (dropped while no device was targeted) send
  them, otherwise open the file picker. ⌘V/Ctrl+V with text on the clipboard
  and focus outside a text field = "Send clipboard" to the selected device.

### Transfer HUD
Per transfer row: title → peer, thin progress bar (4 px, accent; success when
done), `done / total`, speed (`MB/s`, switch to `Gbit/s` above 1000 MB/s),
ETA (`6 s`, `2 min`), path badge, Pause/Resume (outgoing only), Cancel.
States: Connecting… · Waiting for <peer> to accept… · Reconnecting… (show path
badge change on failover) · Verifying… · ✓ Sent / ✓ Saved (click → reveal in
Finder/Explorer/Files) · ✗ <error>.

### Incoming offer (unknown sender)
Native alert/sheet: "<Peer> wants to send you <title> (<size>)" + the four SAS
emoji large, with caption "Check that <Peer> shows the same emoji" and buttons
Decline / Accept.

### Link My Device
Sheet from My Devices empty state or toolbar "+": two tabs — "Show my code"
(calls `open_link_window(300)` and shows code + QR) and "Enter code" (calls
`link_with_code`). Success: device appears in My Devices with a soft spring.

## Copy
All UI text comes from `i18n/strings.json` (keys shared across platforms).
Italian is a first-class language, not a translation afterthought: keep it
short and natural ("Invia", "Dispositivi vicini", "Il tuo codice").
