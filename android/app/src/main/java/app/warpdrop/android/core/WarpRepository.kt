package app.warpdrop.android.core

import android.annotation.SuppressLint
import android.content.Context
import android.os.Environment
import android.util.Log
import app.warpdrop.android.AppVisibility
import app.warpdrop.android.R
import app.warpdrop.android.service.Notifications
import app.warpdrop.android.share.DirectShare
import app.warpdrop.android.share.PreparedSend
import app.warpdrop.android.tile.CodeTileService
import app.warpdrop.android.util.EngineErrors
import app.warpdrop.android.util.UiText
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.drop
import kotlinx.coroutines.flow.filterNotNull
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeoutOrNull
import uniffi.warp.ClipboardContent
import uniffi.warp.CodeInfo
import uniffi.warp.DeviceIdentity
import uniffi.warp.DeviceKind
import uniffi.warp.IncomingOffer
import uniffi.warp.NetInterface
import uniffi.warp.NodeConfig
import uniffi.warp.PeerInfo
import uniffi.warp.TransferSnapshot
import uniffi.warp.TransferState
import uniffi.warp.TrustedDevice
import uniffi.warp.WarpEvent
import uniffi.warp.WarpEventListener
import uniffi.warp.WarpNode
import uniffi.warp.defaultNodeConfig
import java.io.File

/** Where a send goes: a discovered/trusted device, or whoever shows a code / QR payload. */
sealed interface SendTarget {
    data class Peer(val id: String, val name: String) : SendTarget
    data class Code(val code: String) : SendTarget
}

/** Something received via the Share Sheet that waits for a device or a code. */
sealed interface Staged {
    val label: UiText

    class Files(val prepared: PreparedSend) : Staged {
        override val label: UiText get() = prepared.label
    }

    class Clip(val content: ClipboardContent) : Staged {
        override val label: UiText get() = UiText.res(R.string.send_clipboard)
    }
}

sealed interface UiEvent {
    data class Info(val text: UiText) : UiEvent
    data class Error(val text: UiText) : UiEvent
    data class Linked(val device: TrustedDevice) : UiEvent
}

sealed interface NodeStatus {
    data object Stopped : NodeStatus
    data object Starting : NodeStatus
    data object Running : NodeStatus
    data class Failed(val message: String) : NodeStatus
}

/**
 * Process-wide owner of engine state. [app.warpdrop.android.service.WarpService]
 * creates/destroys the single [WarpNode]; everything else observes the
 * StateFlows here and calls the wrappers, which never throw.
 *
 * Holds only the *application* context, so the static reference cannot leak an activity.
 */
@SuppressLint("StaticFieldLeak")
object WarpRepository {
    private const val TAG = "WarpRepository"
    const val TUNNEL_PORT: UShort = 47821u
    private val TERMINAL = setOf(TransferState.COMPLETED, TransferState.FAILED, TransferState.CANCELLED)

    private lateinit var app: Context
    private lateinit var settings: AppSettings
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)

    @Volatile
    private var node: WarpNode? = null
    private val nodeLock = Any()

    private val _node = MutableStateFlow<WarpNode?>(null)
    private val _status = MutableStateFlow<NodeStatus>(NodeStatus.Stopped)
    private val _identity = MutableStateFlow<DeviceIdentity?>(null)
    private val _code = MutableStateFlow<CodeInfo?>(null)
    private val _peers = MutableStateFlow<List<PeerInfo>>(emptyList())
    private val _trusted = MutableStateFlow<List<TrustedDevice>>(emptyList())
    private val _transfers = MutableStateFlow<List<TransferSnapshot>>(emptyList())
    private val _offers = MutableStateFlow<List<IncomingOffer>>(emptyList())
    private val _interfaces = MutableStateFlow<List<NetInterface>>(emptyList())
    private val _downloadDir = MutableStateFlow("")
    private val _staged = MutableStateFlow<Staged?>(null)
    private val _events = MutableSharedFlow<UiEvent>(extraBufferCapacity = 32)

    val nodeFlow: StateFlow<WarpNode?> = _node.asStateFlow()
    val status: StateFlow<NodeStatus> = _status.asStateFlow()
    val identity: StateFlow<DeviceIdentity?> = _identity.asStateFlow()
    val code: StateFlow<CodeInfo?> = _code.asStateFlow()
    val peers: StateFlow<List<PeerInfo>> = _peers.asStateFlow()
    val trusted: StateFlow<List<TrustedDevice>> = _trusted.asStateFlow()

    /** Newest first. */
    val transfers: StateFlow<List<TransferSnapshot>> = _transfers.asStateFlow()
    val offers: StateFlow<List<IncomingOffer>> = _offers.asStateFlow()
    val interfaces: StateFlow<List<NetInterface>> = _interfaces.asStateFlow()
    val downloadDir: StateFlow<String> = _downloadDir.asStateFlow()
    val staged: StateFlow<Staged?> = _staged.asStateFlow()
    val events: SharedFlow<UiEvent> = _events.asSharedFlow()

    // Transfer bookkeeping (guarded by transferLock).
    private val transferLock = Any()
    private val transferMap = LinkedHashMap<String, TransferSnapshot>()
    private val leases = HashMap<String, PreparedSend>()

    fun init(context: Context) {
        app = context.applicationContext
        settings = AppSettings(app)
        // Offers that arrive while the UI is visible show as a dialog; if the
        // user leaves with one pending, fall back to a heads-up notification.
        scope.launch {
            AppVisibility.foreground.drop(1).collect { visible ->
                _offers.value.forEach { offer ->
                    if (visible) Notifications.cancelOffer(app, offer.offerId) else Notifications.showOffer(app, offer)
                }
            }
        }
    }

    val settingsStore: AppSettings get() = settings

    val hasActiveTransfers: Boolean get() = _transfers.value.any { it.state !in TERMINAL }

    fun isTerminal(state: TransferState) = state in TERMINAL

    // ------------------------------------------------------------ lifecycle

    /** Creates the node (blocking; call off the main thread). Idempotent. */
    fun startNode(context: Context): WarpNode? = synchronized(nodeLock) {
        node?.let { return it }
        // A previous node may still be releasing its sockets (tunnel/beacon ports).
        pendingShutdown?.join(5_000)
        _status.value = NodeStatus.Starting
        try {
            val n = WarpNode(buildConfig(context), Listener)
            node = n
            _identity.value = n.identity()
            _code.value = n.currentCode()
            _peers.value = n.peers()
            _trusted.value = n.trustedDevices()
            _interfaces.value = n.interfaces()
            _downloadDir.value = n.downloadDir()
            _node.value = n
            _status.value = NodeStatus.Running
            DirectShare.publish(app, _trusted.value)
            CodeTileService.requestUpdate(app)
            n
        } catch (e: Exception) {
            Log.e(TAG, "WarpNode failed to start", e)
            _status.value = NodeStatus.Failed(e.message ?: e.javaClass.simpleName)
            _events.tryEmit(UiEvent.Error(UiText.res(R.string.error_engine_start, e.message ?: e.javaClass.simpleName)))
            null
        }
    }

    @Volatile
    private var pendingShutdown: Thread? = null

    /**
     * Detaches the node immediately and shuts it down — on a background thread
     * when [async] (the call is then safe from the main thread). [before] runs
     * first on that same thread (e.g. stopping NSD, which still needs the node).
     */
    fun stopNode(async: Boolean, before: () -> Unit = {}) {
        val n = synchronized(nodeLock) {
            val n = node
            node = null
            _node.value = null
            n
        }
        val shutdown = {
            runCatching(before)
            if (n != null) {
                runCatching { n.shutdown() }.onFailure { Log.w(TAG, "shutdown", it) }
                runCatching { n.close() }
            }
        }
        val toClose = synchronized(transferLock) {
            transferMap.clear()
            _transfers.value = emptyList()
            leases.values.toList().also { leases.clear() }
        }
        _offers.value.forEach { Notifications.cancelOffer(app, it.offerId) }
        _offers.value = emptyList()
        _code.value = null
        _peers.value = emptyList()
        _status.value = NodeStatus.Stopped
        CodeTileService.requestUpdate(app)
        if (async) {
            pendingShutdown = Thread({
                shutdown()
                toClose.forEach { it.close() }
            }, "warp-shutdown").also { it.start() }
        } else {
            shutdown()
            toClose.forEach { it.close() }
        }
    }

    suspend fun awaitNode(timeoutMs: Long = 15_000): WarpNode? =
        withTimeoutOrNull(timeoutMs) { _node.filterNotNull().first() }

    private fun buildConfig(context: Context): NodeConfig {
        val stateDir = File(context.filesDir, "warp").apply { mkdirs() }
        val cfg = defaultNodeConfig(settings.deviceName, stateDir.absolutePath, resolveDownloadDir(context).absolutePath)
        cfg.deviceKind = DeviceKind.ANDROID
        cfg.identitySeed = IdentityStore.loadOrCreateSeed(context)
        cfg.enableMdns = false // NsdManager feeds peers via injectPeer
        cfg.enableBeacon = true
        cfg.enableAdb = false
        cfg.tunnelListenPort = TUNNEL_PORT // desktops reach us with `adb forward`
        cfg.relayUrl = settings.relayUrl
        return cfg
    }

    /** Public `Download/WarpDrop` (Android 11+ allows direct paths there), else app-specific storage. */
    fun resolveDownloadDir(context: Context): File {
        @Suppress("DEPRECATION") // Direct paths in public Download are supported on API 30+.
        val public = File(Environment.getExternalStoragePublicDirectory(Environment.DIRECTORY_DOWNLOADS), "WarpDrop")
        if (isWritableDir(public)) return public
        val base = context.getExternalFilesDir(null) ?: context.filesDir
        return File(base, "WarpDrop").apply { mkdirs() }
    }

    private fun isWritableDir(dir: File): Boolean = try {
        (dir.isDirectory || dir.mkdirs()) && File(dir, ".warp-probe-${System.nanoTime()}").let { probe ->
            probe.createNewFile().also { probe.delete() }
        }
    } catch (e: Exception) {
        false
    }

    // ------------------------------------------------------------- events

    private object Listener : WarpEventListener {
        override fun onEvent(event: WarpEvent) {
            try {
                handle(event)
            } catch (e: Exception) {
                Log.e(TAG, "event handling failed: $event", e)
            }
        }
    }

    private fun handle(event: WarpEvent) {
        when (event) {
            is WarpEvent.PeersChanged -> {
                _peers.value = event.peers
                // `online` of trusted devices follows the peer table.
                withNode { _trusted.value = it.trustedDevices() }
            }
            is WarpEvent.CodeChanged -> {
                _code.value = event.code
                CodeTileService.requestUpdate(app)
            }
            is WarpEvent.TrustRingChanged -> {
                _trusted.value = event.devices
                DirectShare.publish(app, event.devices)
            }
            is WarpEvent.OfferReceived -> {
                _offers.update { list -> list.filterNot { it.offerId == event.offer.offerId } + event.offer }
                if (!AppVisibility.isForeground) Notifications.showOffer(app, event.offer)
            }
            is WarpEvent.OfferWithdrawn -> {
                _offers.update { list -> list.filterNot { it.offerId == event.offerId } }
                Notifications.cancelOffer(app, event.offerId)
            }
            is WarpEvent.TransferUpdated -> onTransfer(event.transfer)
            is WarpEvent.ClipboardReceived -> {
                ClipboardBridge.write(app, event.fromName, event.content)
                Notifications.showClipboard(app, event.fromName)
            }
            is WarpEvent.DeviceLinked -> {
                _events.tryEmit(UiEvent.Linked(event.device))
                _events.tryEmit(UiEvent.Info(UiText.res(R.string.link_success, event.device.name)))
            }
            is WarpEvent.Error -> _events.tryEmit(UiEvent.Error(EngineErrors.fromMessage(event.message, null)))
        }
    }

    private fun onTransfer(t: TransferSnapshot) {
        val terminal = t.state in TERMINAL
        var lease: PreparedSend? = null
        val wasTerminal: Boolean
        synchronized(transferLock) {
            val prev = transferMap.put(t.id, t)
            wasTerminal = prev != null && prev.state in TERMINAL
            _transfers.value = sortedTransfers()
            if (terminal) lease = leases.remove(t.id)
        }
        lease?.close()
        if (terminal && !wasTerminal) Notifications.onTransferFinished(app, t, AppVisibility.isForeground)
    }

    /** Newest first by engine start time (caller holds transferLock). */
    private fun sortedTransfers(): List<TransferSnapshot> =
        transferMap.values.sortedByDescending { it.startedAtMs }

    /** Keeps [prepared] open until transfer [id] ends (or closes it now if it already did). */
    private fun attachLease(id: String, prepared: PreparedSend) {
        val closeNow = synchronized(transferLock) {
            val current = transferMap[id]
            if (current != null && current.state in TERMINAL) {
                true
            } else {
                leases[id] = prepared
                false
            }
        }
        if (closeNow) prepared.close()
    }

    // ------------------------------------------------------------ actions

    private inline fun <T> withNode(block: (WarpNode) -> T): T? {
        val n = node ?: return null
        return try {
            block(n)
        } catch (e: IllegalStateException) {
            null // node destroyed concurrently
        }
    }

    /** Takes ownership of [prepared]. Returns the transfer id, or null after emitting an error. */
    fun sendFiles(target: SendTarget, prepared: PreparedSend): String? {
        val n = node
        if (n == null) {
            prepared.close()
            _events.tryEmit(UiEvent.Error(UiText.res(R.string.error_generic, "engine not running")))
            return null
        }
        return try {
            val id = when (target) {
                is SendTarget.Peer -> n.sendFiles(target.id, prepared.items).also { DirectShare.reportUsed(app, target.id) }
                is SendTarget.Code -> n.sendFilesToCode(target.code, prepared.items)
            }
            attachLease(id, prepared)
            id
        } catch (e: Exception) {
            prepared.close()
            _events.tryEmit(UiEvent.Error(EngineErrors.fromException(e, (target as? SendTarget.Peer)?.name)))
            null
        }
    }

    fun sendClipboard(target: SendTarget, content: ClipboardContent): String? {
        val n = node ?: return null
        return try {
            when (target) {
                is SendTarget.Peer -> n.sendClipboard(target.id, content).also { DirectShare.reportUsed(app, target.id) }
                is SendTarget.Code -> n.sendClipboardToCode(target.code, content)
            }
        } catch (e: Exception) {
            _events.tryEmit(UiEvent.Error(EngineErrors.fromException(e, (target as? SendTarget.Peer)?.name)))
            null
        }
    }

    /** Sends whatever is staged; returns false when nothing was staged. */
    fun sendStaged(target: SendTarget): Boolean {
        val staged = _staged.getAndUpdateCompat() ?: return false
        when (staged) {
            is Staged.Files -> sendFiles(target, staged.prepared)
            is Staged.Clip -> sendClipboard(target, staged.content)
        }
        return true
    }

    private fun MutableStateFlow<Staged?>.getAndUpdateCompat(): Staged? {
        while (true) {
            val cur = value
            if (compareAndSet(cur, null)) return cur
        }
    }

    fun stage(item: Staged) {
        val old = _staged.value
        _staged.value = item
        if (old is Staged.Files && old !== item) old.prepared.close()
    }

    fun clearStaged() {
        val old = _staged.getAndUpdateCompat()
        if (old is Staged.Files) old.prepared.close()
    }

    fun pause(id: String) = guard { it.pauseTransfer(id) }
    fun resume(id: String) = guard { it.resumeTransfer(id) }
    fun cancel(id: String) = guard { it.cancelTransfer(id) }

    fun cancelAllActive() {
        _transfers.value.filter { it.state !in TERMINAL }.forEach { cancel(it.id) }
    }

    fun clearFinished() {
        guard { it.clearFinishedTransfers() }
        synchronized(transferLock) {
            transferMap.values.removeAll { it.state in TERMINAL }
            _transfers.value = sortedTransfers()
        }
    }

    fun respondToOffer(offerId: String, accept: Boolean) {
        guard { it.respondToOffer(offerId, accept) }
        _offers.update { list -> list.filterNot { it.offerId == offerId } }
        Notifications.cancelOffer(app, offerId)
    }

    fun untrust(deviceId: String) = guard { it.untrustDevice(deviceId) }

    fun openLinkWindow(seconds: UInt) = guard { it.openLinkWindow(seconds) }
    fun closeLinkWindow() = guard { it.closeLinkWindow() }

    /** The result arrives as [UiEvent.Linked] or [UiEvent.Error]. */
    fun linkWithCode(code: String): Boolean = guard { it.linkWithCode(code) }

    fun setDeviceName(name: String) {
        val clean = name.trim()
        if (clean.isEmpty()) return
        settings.deviceName = clean
        withNode {
            it.setDeviceName(clean)
            _identity.value = it.identity()
        }
    }

    /** The core restarts its rendezvous listener live. */
    fun setRelayUrl(url: String?) {
        val clean = url?.trim()?.takeIf { it.isNotEmpty() }
        settings.relayUrl = clean
        if (guard { it.setRelayUrl(clean) } || node == null) {
            _events.tryEmit(UiEvent.Info(UiText.res(R.string.relay_applied)))
        }
    }

    fun injectPeer(advert: uniffi.warp.PeerAdvert) = guard { it.injectPeer(advert) }
    fun removeInjectedPeer(deviceId: String) = guard { it.removeInjectedPeer(deviceId) }
    fun advertisement(): uniffi.warp.ServiceAdvert? = withNode { it.advertisement() }

    fun refreshNetwork() {
        withNode {
            _interfaces.value = it.interfaces()
            _peers.value = it.peers()
            _trusted.value = it.trustedDevices()
        }
    }

    fun emit(event: UiEvent) {
        _events.tryEmit(event)
    }

    /** Runs [block] against the node, reporting engine errors; true on success. */
    private inline fun guard(block: (WarpNode) -> Unit): Boolean {
        val n = node ?: return false
        return try {
            block(n)
            true
        } catch (e: IllegalStateException) {
            false
        } catch (e: Exception) {
            _events.tryEmit(UiEvent.Error(EngineErrors.fromException(e, null)))
            false
        }
    }
}
