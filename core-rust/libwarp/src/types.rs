//! Plain data types shared by the engine and every platform UI (via UniFFI).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, uniffi::Enum)]
pub enum DeviceKind {
    Mac,
    IPhone,
    IPad,
    Android,
    Windows,
    Linux,
    Unknown,
}

impl DeviceKind {
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            DeviceKind::Mac
        } else if cfg!(target_os = "ios") {
            DeviceKind::IPhone
        } else if cfg!(target_os = "android") {
            DeviceKind::Android
        } else if cfg!(target_os = "windows") {
            DeviceKind::Windows
        } else if cfg!(target_os = "linux") {
            DeviceKind::Linux
        } else {
            DeviceKind::Unknown
        }
    }
    pub fn as_tag(&self) -> &'static str {
        match self {
            DeviceKind::Mac => "mac",
            DeviceKind::IPhone => "iphone",
            DeviceKind::IPad => "ipad",
            DeviceKind::Android => "android",
            DeviceKind::Windows => "windows",
            DeviceKind::Linux => "linux",
            DeviceKind::Unknown => "unknown",
        }
    }
    pub fn from_tag(s: &str) -> Self {
        match s {
            "mac" => DeviceKind::Mac,
            "iphone" => DeviceKind::IPhone,
            "ipad" => DeviceKind::IPad,
            "android" => DeviceKind::Android,
            "windows" => DeviceKind::Windows,
            "linux" => DeviceKind::Linux,
            _ => DeviceKind::Unknown,
        }
    }
}

/// The physical/logical link a connection runs over, ordered by preference
/// (lower tier = preferred).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, uniffi::Enum)]
pub enum PathKind {
    /// USB-NCM / RNDIS / CDC-Ethernet / iPhone USB point-to-point link.
    UsbCable,
    /// Thunderbolt bridge (`bridge0`) between two Macs or a Mac and a PC.
    ThunderboltBridge,
    /// Android device reached through an `adb forward` tunnel over USB.
    AdbUsb,
    /// Apple Wireless Direct Link / Wi-Fi peer-to-peer.
    WifiDirect,
    /// Wired LAN.
    Ethernet,
    /// Wireless LAN through an access point.
    Wifi,
    /// Local network of unknown medium.
    Lan,
    /// Same machine (development / tests).
    Loopback,
    /// Internet, direct UDP after NAT hole punching.
    InternetP2p,
    /// Internet, end-to-end encrypted packets forwarded by the relay.
    Relay,
}

impl PathKind {
    /// Tier 0 = cable, 1 = local wireless/LAN, 2 = internet.
    pub fn tier(&self) -> u8 {
        match self {
            PathKind::UsbCable | PathKind::ThunderboltBridge | PathKind::AdbUsb => 0,
            PathKind::Loopback => 0,
            PathKind::Ethernet | PathKind::WifiDirect | PathKind::Wifi | PathKind::Lan => 1,
            PathKind::InternetP2p => 2,
            PathKind::Relay => 3,
        }
    }
    /// Finer-grained ordering inside a tier.
    pub fn rank(&self) -> u8 {
        match self {
            PathKind::Loopback => 0,
            PathKind::ThunderboltBridge => 1,
            PathKind::UsbCable => 2,
            PathKind::AdbUsb => 3,
            PathKind::Ethernet => 10,
            PathKind::WifiDirect => 11,
            PathKind::Wifi => 12,
            PathKind::Lan => 13,
            PathKind::InternetP2p => 20,
            PathKind::Relay => 30,
        }
    }
    pub fn is_wired(&self) -> bool {
        self.tier() == 0 && *self != PathKind::Loopback
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct NodeConfig {
    /// Human readable name shown to other devices ("Giuseppe's MacBook Pro").
    pub device_name: String,
    pub device_kind: DeviceKind,
    /// Directory for identity, trust ring and resume state.
    pub state_dir: String,
    /// Where received files are saved.
    pub download_dir: String,
    /// 32-byte Ed25519 seed supplied by the platform keychain/keystore. When
    /// absent the core generates one and persists it in `state_dir`.
    pub identity_seed: Option<Vec<u8>>,
    /// UDP port for the QUIC endpoint (0 = ephemeral).
    pub port: u16,
    /// Use the built-in mDNS responder/browser. Apple and Android disable this
    /// and feed peers in via `inject_peer` from Bonjour / NsdManager instead.
    pub enable_mdns: bool,
    /// UDP broadcast beacon fallback for routers that filter multicast.
    pub enable_beacon: bool,
    /// UDP port used by the broadcast beacon.
    pub beacon_port: u16,
    /// Probe for Android devices connected over USB through `adb`.
    pub enable_adb: bool,
    /// Accept QUIC-over-TCP tunnels (the target of `adb forward`) on this
    /// loopback port. Android enables this.
    pub tunnel_listen_port: Option<u16>,
    /// Rendezvous/relay server, e.g. `wss://relay.example.com` or `ws://10.0.0.2:47900`.
    pub relay_url: Option<String>,
    /// Skip hole punching and always use the relay (diagnostics / tests).
    pub force_relay: bool,
    /// How long a Wormhole code stays valid before rotating.
    pub code_rotation_secs: u32,
    /// Parallel QUIC data streams per transfer.
    pub parallel_streams: u8,
    pub auto_accept_trusted: bool,
}

impl NodeConfig {
    pub fn with_dirs(name: &str, state_dir: &str, download_dir: &str) -> Self {
        NodeConfig {
            device_name: name.to_string(),
            device_kind: DeviceKind::current(),
            state_dir: state_dir.to_string(),
            download_dir: download_dir.to_string(),
            identity_seed: None,
            port: 0,
            enable_mdns: true,
            enable_beacon: true,
            beacon_port: crate::discovery::DEFAULT_BEACON_PORT,
            enable_adb: false,
            tunnel_listen_port: None,
            relay_url: None,
            force_relay: false,
            code_rotation_secs: 600,
            parallel_streams: 6,
            auto_accept_trusted: true,
        }
    }
}

#[uniffi::export]
pub fn default_node_config(device_name: String, state_dir: String, download_dir: String) -> NodeConfig {
    NodeConfig::with_dirs(&device_name, &state_dir, &download_dir)
}

#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct DeviceIdentity {
    pub device_id: String,
    pub name: String,
    pub kind: DeviceKind,
    pub public_key_hex: String,
    /// Human-comparable fingerprint, e.g. `3F9A 11C2 8B70 04DE`.
    pub fingerprint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct PeerInfo {
    pub device_id: String,
    pub name: String,
    pub kind: DeviceKind,
    pub trusted: bool,
    /// The device is currently accepting Wormhole-code connections.
    pub code_open: bool,
    pub best_path: Option<PathKind>,
    pub addresses: Vec<String>,
    pub last_seen_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
pub enum TrustOrigin {
    /// Synced from the same iCloud account.
    ICloud,
    /// Linked once with "Link My Device".
    Linked,
    /// Learned from another device in the ring.
    Ring,
}

#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct TrustedDevice {
    pub device_id: String,
    pub name: String,
    pub kind: DeviceKind,
    pub public_key_hex: String,
    pub origin: TrustOrigin,
    pub added_at_ms: u64,
    pub online: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct CodeInfo {
    /// Six digits, no separator.
    pub code: String,
    /// `482-910`
    pub display: String,
    pub expires_at_ms: u64,
    /// Payload for the QR code (`warpdrop:` URI with code + direct addresses).
    pub qr_payload: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct QrMatrix {
    pub size: u32,
    /// Row-major, `size * size` modules; true = dark.
    pub modules: Vec<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct SendItem {
    /// Filesystem path. On Android `/proc/self/fd/N` works for content URIs.
    pub path: String,
    /// Optional name override (used when `path` is not meaningful). May be a
    /// `/`-separated relative path (`"Photos/2025/a.jpg"`) to place a file
    /// inside a folder, so platforms can stream a picked folder file by file.
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, uniffi::Enum)]
pub enum ClipboardContent {
    Text { text: String },
    Url { url: String },
    Image { mime: String, data: Vec<u8> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
pub enum TransferDirection {
    Outgoing,
    Incoming,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
pub enum TransferState {
    Connecting,
    WaitingForAccept,
    Transferring,
    Paused,
    Reconnecting,
    Verifying,
    Completed,
    Failed,
    Cancelled,
}

impl TransferState {
    pub fn is_terminal(&self) -> bool {
        matches!(self, TransferState::Completed | TransferState::Failed | TransferState::Cancelled)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct TransferSnapshot {
    pub id: String,
    pub direction: TransferDirection,
    pub peer_id: String,
    pub peer_name: String,
    /// "Holiday Photos" or "3 items".
    pub title: String,
    pub file_count: u64,
    pub total_bytes: u64,
    pub done_bytes: u64,
    /// Bytes that did not need to be sent because the receiver already had them.
    pub resumed_bytes: u64,
    /// Bytes actually put on the wire (after compression).
    pub wire_bytes: u64,
    pub speed_bps: u64,
    pub eta_secs: Option<u64>,
    pub path: Option<PathKind>,
    pub state: TransferState,
    /// Human-readable (English) failure description.
    pub error: Option<String>,
    /// Stable machine-readable failure code for localisation:
    /// `wrong_code`, `code_not_found`, `declined`, `rejected`, `incompatible`,
    /// `unreachable`, `timeout`, `cancelled`, `io`, `integrity`, `protocol`.
    pub error_code: Option<String>,
    /// When the transfer was created (Unix ms) — for stable list ordering.
    pub started_at_ms: u64,
    pub saved_paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct IncomingOffer {
    pub offer_id: String,
    pub peer_id: String,
    pub peer_name: String,
    pub peer_kind: DeviceKind,
    pub trusted: bool,
    pub item_count: u64,
    pub total_bytes: u64,
    pub title: String,
    /// Four emoji both screens show; compare them when the sender is unknown.
    pub sas_emoji: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct NetInterface {
    pub name: String,
    pub address: String,
    pub kind: PathKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct ServiceAdvert {
    pub service_type: String,
    pub instance_name: String,
    pub port: u16,
    pub txt: HashMap<String, String>,
}

/// A peer seen by platform-native discovery (Bonjour / NsdManager).
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct PeerAdvert {
    pub txt: HashMap<String, String>,
    /// `ip:port` strings; IPv6 link-local may carry a `%scope`.
    pub addresses: Vec<String>,
}

/// JSON form (desktop webview): `{"type": "transferUpdated", "transfer": {...}}`.
#[derive(Debug, Clone, Serialize, uniffi::Enum)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum WarpEvent {
    PeersChanged { peers: Vec<PeerInfo> },
    CodeChanged { code: CodeInfo },
    TrustRingChanged { devices: Vec<TrustedDevice> },
    OfferReceived { offer: IncomingOffer },
    OfferWithdrawn { offer_id: String },
    TransferUpdated { transfer: TransferSnapshot },
    ClipboardReceived { from_device_id: String, from_name: String, content: ClipboardContent },
    DeviceLinked { device: TrustedDevice },
    Error { message: String },
}

/// Implemented by each platform UI to receive engine events. Called from a
/// dedicated engine thread; implementations should hop to their UI thread.
#[uniffi::export(with_foreign)]
pub trait WarpEventListener: Send + Sync {
    fn on_event(&self, event: WarpEvent);
}
