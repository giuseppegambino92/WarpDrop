# WarpDrop — architecture, security model & verification

End-to-end encrypted file, folder and clipboard transfer between **macOS, iOS,
Android, Windows and Linux** — on the same desk (USB / Thunderbolt), the same
network (Wi-Fi, Ethernet, AWDL), or across the Internet (hole punching or an
end-to-end encrypted relay).

```
core-rust/libwarp        Rust engine (discovery, QUIC/TLS 1.3, SPAKE2, streaming, resume) + UniFFI
core-rust/warp-cli       `warp` headless node (scripting, diagnostics)
core-rust/uniffi-bindgen bindings generator
rendezvous-relay         `warp-signal`: rendezvous + STUN-style discovery + UDP relay
apple/                   Swift 6 / SwiftUI: macOS app + menu-bar droplet, iOS app, Share extensions, Live Activity
android/                 Kotlin / Jetpack Compose (Material 3): app, foreground service, share target, QS tile
desktop/                 Tauri v2 (Rust + React/TS + Tailwind): Windows / Linux (also runs on macOS)
docs/design-system.md    shared visual language;  i18n/strings.json  shared EN/IT strings
```

## How it works

### Paths (ranked automatically, best first)
| Tier | Path | How it is found |
|---|---|---|
| 0 | **⚡ USB cable** — USB-NCM/RNDIS/CDC tethering, iPhone USB (`172.20.10.x`), Android tethering (`192.168.42.x`), link-local `169.254/16` | interface scanner (`netif.rs`: subnets, names, Linux drivers, macOS hardware-port table) |
| 0 | **⚡ Thunderbolt bridge** (`bridge0`) | interface scanner |
| 0 | **⚡ Android over ADB** — QUIC through `adb forward` (no network needed) | desktop probes `adb devices`; phone listens on `127.0.0.1:47821` |
| 1 | Ethernet · **Wi-Fi Direct / AWDL** · Wi-Fi · LAN | mDNS `_warpdrop._udp`, UDP broadcast beacon, Bonjour `NWBrowser` with `includePeerToPeer` (Apple), `NsdManager` (Android) |
| 2 | **☁️ P2P** — QUIC after UDP hole punching | `warp-signal` rendezvous by code |
| 3 | **☁️ Relay** — QUIC packets forwarded by `warp-signal` (it never has keys) | fallback when NAT is symmetric |

Candidates are raced happy-eyeballs style (best first, next one 250 ms later).
If a connection drops mid-transfer the sender reconnects over **any** path and
resumes (see below).

On LAN/cable paths the same QUIC session is offered over two carriages: a TCP
stream with 16 KiB QUIC packets (tried first: kernel TCP offloads, far fewer
per-packet operations) and native QUIC/UDP. Internet paths are UDP only.
Encryption and authentication are identical on both.

### Security
* **Identity**: one Ed25519 key per device (seed in Keychain / Android
  Keystore-wrapped / 0600 file on desktop), presented as a self-signed
  certificate in **TLS 1.3 only**; suites AES-256-GCM → ChaCha20-Poly1305
  (AES-128-GCM only for QUIC Initial packets, as RFC 9001 requires).
* **Personal Trust Ring** (My Devices): pinned keys → mutual TLS, zero prompts.
  Apple devices on the same iCloud account exchange public keys through
  `NSUbiquitousKeyValueStore`; any other device joins once with
  **Link My Device** (receiver opens a link window, sender types its code).
  Ring membership propagates between trusted devices.
* **Wormhole code** (`482-910`, rotating, single use, 3 wrong guesses → new
  code): **SPAKE2** whose transcript includes the **TLS exporter**, followed by
  explicit key confirmation. A man in the middle sees two different TLS
  sessions, so the keys diverge: one online guess (1 in 10⁶) per attempt.
  The QR code carries the same code plus addresses and the pinned key.
* **Unknown nearby device** (no code): the receiver is asked, and both screens
  show 4 SAS emoji derived from the TLS exporter.
* **Relay**: sees an Argon2id hash of the code (memory-hard, so codes can't be
  reversed within their lifetime), public addresses, and QUIC ciphertext.
  It is hardened against abuse: channels are first come, first served (a live
  listener is never replaced), bounded per-client queues (a client that stops
  draining essential messages is disconnected rather than silently missing
  them), per-connection
  `Connect` rate limit, per-IP WebSocket cap, global session/listener caps,
  30 s expiry for sessions that never carried traffic, and WebSocket pings to
  free channels of dead clients. A UDP relay side can only move to a new
  address with a **BIND** packet carrying `BLAKE3-keyed(bind_key, session,
  side, observed address)`, where `bind_key` is handed to that client alone
  over the WebSocket; knowing the session id is not enough to claim, redirect
  or cut off a side, not even before its first binding. v1.0.0 clients have no
  bind key: for them, start the relay with `WARP_SIGNAL_LEGACY_BIND=1` (their
  first packet then binds; nothing can move it afterwards without the key).
* **Integrity**: every 4 MiB chunk is a BLAKE3 subtree verified on arrival;
  per-file roots are the standard BLAKE3 hash (`b3sum`-identical) and must
  match before anything leaves the staging area. Manifest paths are validated
  (no traversal, Windows-reserved names sanitised, every parent folder
  declared), and names that would land on the same file after sanitising,
  Unicode normalization or case folding are renamed `name (2)` instead.
  Incoming manifests have a memory budget sized to the device: by default
  1/32 of RAM (32 MiB – 1 GiB; `NodeConfig.transfer_memory_mb` overrides it),
  a quarter for per-chunk state (~80 B peak per chunk) and the rest for
  entries; e.g. a 3 GB phone takes up to ~73 k entries and ~1.2 TiB, a 16 GB
  Mac 393 k entries and the 4 TiB hard cap (2²⁰ chunks, so the list of
  already-held chunks always fits one control message; 1 M entries is the
  other hard cap). The `.chunks` resume log is streamed, read only up to twice
  the chunk count, and compacted to the verified records on every resume. Transfers
  over budget, or larger than the free disk space, are declined before any
  data moves (`too_large` / `no_space`).
* **Hand-off**: verified items move into the download folder with an atomic
  no-replace rename (`renamex_np` / `renameat2` / `MoveFileExW`), so an
  existing file is never overwritten. Each move is atomic; the set is all or
  nothing on a best-effort basis: if one move fails the others are moved back
  into staging, and if a move back fails too, the error lists what stayed in
  the download folder. A write-ahead `.placed` journal in the staging area
  names each destination *before* its rename (temp file, fsync, rename, fsync
  of the folder); if it cannot be written, nothing moves. On the next attempt
  an entry counts only if its item has left staging and is intact at the
  destination, so sending the same items again re-verifies saved items in
  place and neither re-sends nor duplicates them (an item edited in the
  meantime is left untouched and a fresh copy lands next to it). Before the
  sender is told "done", the download folder is synced (renames are
  write-through on Windows) and the staging area is marked complete; it then
  holds only the journal and chunk log for 30 minutes, so a retry of the same
  finished transfer is answered without a second copy.
  *Tested*: a child process is killed with `abort()` at every step of the
  hand-off (before/after each journal write, after each rename, after the
  sync, after the mark); each time the next attempt finds every chunk
  accounted for and ends with each item saved exactly once and existing files
  untouched. Power loss is not simulated: there, durability rests on the
  filesystem honouring fsync (`F_FULLFSYNC` on Apple platforms). On filesystems without
  an atomic no-replace rename (FAT/exFAT, some network/FUSE mounts) files fall
  back to hard links (still atomic) and folders to check-then-rename.

### Transfer engine
* Folders stream **on the fly** (no zip): manifest + chunk frames over N
  parallel QUIC streams, any order, any stream.
* **Adaptive compression**: zstd level 1 per chunk; skipped for media/archives
  by extension and magic bytes, and disabled per file when it saves < 5%.
* **Resume, on any path, with zero prompts**: the receiver stages into
  `.warpdrop-partial/`, logs verified chunks, re-hashes them from disk on
  reconnect, and the sender skips every chunk whose content hash matches.
  With its `Accept` the receiver issues a **resumption ticket** (256-bit,
  bound to the sender's TLS-proven Ed25519 key *and* the manifest hash, valid
  30 minutes) and listens on the rendezvous server under a keyed BLAKE3 hash
  of it. A dropped Internet transfer therefore reconnects through `warp-signal`
  by itself — no new 6-digit code — while the sender pins the receiver's key.
  A stolen ticket is useless without the sender's private key, and it covers
  exactly one manifest.
* **Clipboard push**: text, URLs, images (≤ 32 MiB) straight into the other
  device's clipboard.
* Pause / resume / cancel; speed, ETA and the active path are live events.

## Verified results (this repository, macOS 27 / Apple silicon)

| Check | Result |
|---|---|
| `cargo test --workspace` | **66 passed**: libwarp 36 unit + 11 end-to-end + 2 resume + 3 hand-off + 7 relay abuse, warp-signal 1, desktop 6 |
| `cargo clippy --workspace --all-targets` | 0 warnings |
| End-to-end tests | code reaches a late-discovered device among decoys · LAN discovery (mDNS + beacon) · code pairing + nested folder bit-for-bit (BLAKE3) · QR ticket · wrong code rejected + rotation · Link My Device + zero-prompt send + ring gossip · stranger prompt + SAS · clipboard · Internet rendezvous direct **and** forced relay · ADB-style USB tunnel · pause/resume/cancel |
| Resume tests | LAN: connection dropped after 6/11 chunks → automatic reconnect, ≥ 20 MiB skipped, each byte received once. **Internet**: same drop on a rendezvous-only session (hole-punched *and* forced relay) → reconnects via the resumption ticket, no new code, no prompt |
| Throughput (`cargo run --release -p libwarp --example bench -- 1024 1`) | **854 MB/s** end to end over loopback, 1 GiB incl. BLAKE3 verify + disk |
| Cross-process smoke test | `warp-signal` + `warp receive` + `warp send` (365 MB tree) — hashes identical |
| Apple | `WarpDrop-macOS` and `WarpDrop-iOS` schemes build, 0 warnings in app code; CLI → macOS app transfer verified |
| Android | `assembleDebug` 0 warnings in app code, lint 0 errors; launched on API 36 emulator, share-sheet → engine verified |
| Desktop | `npm run build`, `cargo clippy -p warpdrop-desktop` 0 warnings, macOS `.app` bundle builds; CLI → app transfer verified |

Not verified here: Windows/Linux builds (no toolchains on this Mac — the
release workflow's first run is their first compile), a real two-phone
transfer, physical USB/Thunderbolt/AWDL links, signed Apple builds (iCloud sync
and Keychain sharing need a team ID). The CI macOS job (macOS-only core,
universal Release build, `.dmg`) and Android release signing were rehearsed
locally.

## Live devices (`scripts/dev-launch.sh`)

```sh
./scripts/dev-launch.sh                         # warp-signal on :47900 + the macOS app wired to it
./scripts/dev-launch.sh --signal wss://relay.example.com
./scripts/dev-launch.sh --deploy-ios --team ABCDE12345   # build, install, launch on connected iPhones/iPads
./scripts/dev-launch.sh --deploy-android                 # build, install, launch on `adb devices`
```
It prints the macOS app's code, a ready-to-paste CLI test command, and the
exact `xcrun devicectl` / `adb` commands for every connected physical device
(Simulators are filtered out). Ctrl+C stops everything it started; logs are in
`target/dev-launch/`. Devices on *other* networks need the signaling server to
be reachable from them (port-forward TCP+UDP 47900 or deploy the Docker image).

## Releases (`.github/workflows/release.yml`)

On every `v*` tag (or manually): Rust clippy + tests on Linux, then in
parallel Windows `.msi` + NSIS `.exe` and Linux `.deb` + `.AppImage` (Tauri),
macOS `.dmg` (native Swift app, universal), Android `.apk`, and a
`warp-signal` Linux tarball; a tag publishes a **draft** GitHub Release with
`SHA256SUMS.txt`. Every platform builds unsigned when its secrets are absent:

| Platform | Secrets |
|---|---|
| macOS | `MACOS_CERTIFICATE_P12`, `MACOS_CERTIFICATE_PASSWORD`, `MACOS_SIGNING_IDENTITY`, `APPLE_TEAM_ID`; notarization: `APPLE_ID`, `APPLE_APP_PASSWORD`; iCloud sync + shared keychain: `MACOS_PROVISIONING_PROFILE` |
| Windows | `WINDOWS_CERTIFICATE` (base64 .pfx), `WINDOWS_CERTIFICATE_PASSWORD` |
| Android | `ANDROID_KEYSTORE` (base64 .jks), `ANDROID_KEYSTORE_PASSWORD`, `ANDROID_KEY_ALIAS`, `ANDROID_KEY_PASSWORD` (without them CI ships the debug-signed APK) |

## Quick start (CLI)

```sh
cargo build --release -p warp-cli -p warp-signal
./target/release/warp receive                      # shows "Your code: 482-910"
./target/release/warp send 482-910 ~/Pictures/Trip # on another machine on the LAN
./target/release/warp clip 482-910 "hello"         # clipboard push
./target/release/warp peers | ifaces | hash FILE
```
Across networks: run `warp-signal` somewhere reachable
(`docker build -f rendezvous-relay/Dockerfile -t warp-signal . && docker run -p 47900:47900/tcp -p 47900:47900/udp warp-signal`)
and pass `--relay ws://host:47900` (or set it in each app's settings).
Two CLI nodes on one machine need separate `WARP_STATE` directories.

## Apple (macOS + iOS)
Requirements: Xcode 16+, XcodeGen (`brew install xcodegen`), Rust.

1. `./scripts/build-apple.sh` — builds the Rust core XCFramework + Swift bindings (required on a fresh clone, ~8 min).
2. Optional: `python3 apple/scripts/generate-strings.py` (after editing `i18n/strings.json`), `cd apple && xcodegen generate` (after editing `project.yml`).
3. Open `apple/WarpDrop.xcodeproj`, pick `WarpDrop-macOS` or `WarpDrop-iOS`, set your team, Run.
   Headless: `xcodebuild -project apple/WarpDrop.xcodeproj -scheme WarpDrop-macOS -destination 'generic/platform=macOS' CODE_SIGNING_ALLOWED=NO build`
   (iOS: `-scheme WarpDrop-iOS -destination 'generic/platform=iOS Simulator'`).

The macOS app is a window **and** a menu-bar droplet: drag files onto the
droplet and the device radar pops open. It ships unsandboxed with hardened
runtime (Developer ID) because it runs `adb` and writes to user-chosen
folders; the share extension is sandboxed. Signing needs the App Group
`group.com.warpdrop.shared` (iOS) / `<TEAMID>.com.warpdrop.shared` (macOS),
keychain group `<TEAMID>.com.warpdrop.shared` and the iCloud key-value store.
Unsigned builds run without iCloud sync. Debug: `-WarpDebugPrintCode YES`.

## Android
Requirements: Android SDK (platform + build-tools 37), JDK 17+ (Android Studio's JBR works), `cargo-ndk` for the core.

```sh
./scripts/build-android.sh                        # libwarp.so (arm64-v8a, armeabi-v7a, x86_64) + Kotlin bindings
cd android
echo "sdk.dir=$HOME/Library/Android/sdk" > local.properties
export JAVA_HOME="/Applications/Android Studio.app/Contents/jbr/Contents/Home"
./gradlew :app:assembleDebug                      # app/build/outputs/apk/debug/app-debug.apk
adb install -r app/build/outputs/apk/debug/app-debug.apk
```
Strings are generated from `i18n/strings.json` on every build. USB cable from a
desktop: the desktop app finds the phone through `adb` automatically (the
phone listens for QUIC-over-TCP on `127.0.0.1:47821` while WarpDrop is open).

## Desktop (Windows / Linux)
Tauri v2 app in `desktop/` linking `libwarp` directly.

Prerequisites: Rust, Node 20.19+/22.12+.
* **Windows 10/11**: MSVC Build Tools ("Desktop development with C++"), WebView2 runtime (bundled bootstrapper). Mica on Windows 11, Acrylic on 10.
* **Debian/Ubuntu**: `sudo apt install build-essential curl wget file libssl-dev libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev`
* **Fedora**: `sudo dnf install webkit2gtk4.1-devel gtk3-devel libappindicator-gtk3-devel librsvg2-devel openssl-devel libxdo-devel`
* **Arch**: `sudo pacman -S webkit2gtk-4.1 gtk3 libappindicator-gtk3 librsvg openssl xdotool base-devel`
* Optional: `adb` on PATH for USB transfers to Android.

```sh
cd desktop && npm install
npm run tauri dev                                  # hot reload
npm run tauri build -- --bundles nsis,msi          # Windows
npm run tauri build -- --bundles deb,appimage,rpm  # Linux
```
Debug env: `WARPDROP_PROFILE_DIR`, `WARPDROP_DOWNLOAD_DIR`, `WARPDROP_DEBUG_PRINT_CODE=1`.

## Design decisions & deviations from the original brief
* **SPAKE2, not SPAKE2+.** SPAKE2+ is the *augmented* PAKE whose extra property
  (a server storing a verifier instead of the password) does not exist for a
  short-lived code shown on both screens. Symmetric SPAKE2 bound to the TLS
  exporter gives the intended guarantee.
* **Ed25519 keys are not in the Secure Enclave / StrongBox**: those support
  P-256 only (and could not sign inside rustls). Seeds are protected at rest by
  the Keychain / an Android-Keystore AES-GCM key instead.
* **Menu-bar drop target** is an `NSStatusItem` + `NSPopover`; `MenuBarExtra`
  cannot receive drags.
* **iPhone ↔ Mac over USB** uses the USB-NCM / tethering IP interface (Bonjour
  works over it) rather than talking to `usbmuxd` directly, which is a private
  protocol.
* **ADB tunnels carry QUIC over TCP**, because `adb forward` only forwards TCP.
* **LAN prefers a TCP carriage for QUIC.** On this development Mac (which runs
  a VPN network extension), each UDP datagram cost ~200 µs: loopback UDP ran
  at 7 MB/s while TCP ran at 8 GB/s. Userspace QUIC over UDP also gets no
  segmentation offload on macOS/Windows. The engine therefore offers the same
  QUIC session over a TCP stream first on LAN/cable paths (4 MB/s → 854 MB/s
  here), with native UDP as the fallback. Internet paths stay UDP.
* **Resumption tickets bind to the sender's device key**, which is long-lived
  rather than per-session ephemeral; the per-session secrecy comes from TLS 1.3
  (ephemeral ECDHE), and the binding is to the key *proven in this handshake*.
  Tickets live in memory: they survive network changes, not an app restart.
* **Ring removals don't propagate** (no tombstones): removing a device from My
  Devices is local to that device.
