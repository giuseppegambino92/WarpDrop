package app.warpdrop.android.ui

import android.app.Application
import android.net.Uri
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import app.warpdrop.android.R
import app.warpdrop.android.core.ClipboardBridge
import app.warpdrop.android.core.SendTarget
import app.warpdrop.android.core.UiEvent
import app.warpdrop.android.core.WarpRepository
import app.warpdrop.android.share.SendPreparer
import app.warpdrop.android.share.UnreadableItemException
import app.warpdrop.android.util.EngineErrors
import app.warpdrop.android.util.Locales
import app.warpdrop.android.util.UiText
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.receiveAsFlow
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import uniffi.warp.DeviceKind
import uniffi.warp.PathKind
import uniffi.warp.normalizeCode

enum class Tab { MY_DEVICES, NEARBY, QUICK_CODE }
enum class Screen { HOME, SETTINGS, SCANNER }
enum class PickKind { FILES, FOLDER }
enum class LinkMode { SHOW, ENTER }

data class DeviceItem(
    val id: String,
    val name: String,
    val kind: DeviceKind,
    val trusted: Boolean,
    val online: Boolean,
    val path: PathKind?,
)

data class LinkSheetState(
    val mode: LinkMode = LinkMode.SHOW,
    val windowExpiresAtMs: Long = 0,
    val input: String = "",
    val busy: Boolean = false,
    val error: UiText? = null,
)

class MainViewModel(application: Application) : AndroidViewModel(application) {
    private val app = application
    private val repo = WarpRepository
    private val preparer = SendPreparer(application)

    val code = repo.code
    val identity = repo.identity
    val transfers = repo.transfers
    val offers = repo.offers
    val staged = repo.staged
    val status = repo.status
    val downloadDir = repo.downloadDir
    val interfaces = repo.interfaces
    val events = repo.events

    private val _tab = MutableStateFlow(Tab.MY_DEVICES)
    val tab: StateFlow<Tab> = _tab.asStateFlow()

    private val _screen = MutableStateFlow(Screen.HOME)
    val screen: StateFlow<Screen> = _screen.asStateFlow()

    /** Digits only (0–6); the field renders the dash. */
    private val _codeInput = MutableStateFlow("")
    val codeInput: StateFlow<String> = _codeInput.asStateFlow()

    /** Full `warpdrop://…` payload when one was pasted/scanned (carries direct addresses). */
    private var codePayload: String? = null

    private val _preparing = MutableStateFlow(false)
    val preparing: StateFlow<Boolean> = _preparing.asStateFlow()

    private val _link = MutableStateFlow<LinkSheetState?>(null)
    val link: StateFlow<LinkSheetState?> = _link.asStateFlow()

    private val _qrVisible = MutableStateFlow(false)
    val qrVisible: StateFlow<Boolean> = _qrVisible.asStateFlow()

    private val picks = Channel<PickKind>(Channel.BUFFERED)
    val pickRequests = picks.receiveAsFlow()

    private var pendingTarget: SendTarget? = null

    val myDevices: StateFlow<List<DeviceItem>> = combine(repo.trusted, repo.peers) { trusted, peers ->
        val byId = peers.associateBy { it.deviceId }
        trusted.map { d ->
            val p = byId[d.deviceId]
            DeviceItem(d.deviceId, d.name, d.kind, trusted = true, online = d.online || p != null, path = p?.bestPath)
        }.sortedWith(compareByDescending<DeviceItem> { it.online }.thenBy { it.name.lowercase() })
    }.stateIn(viewModelScope, SharingStarted.Eagerly, emptyList())

    /** Untrusted devices seen on the network. */
    val nearby: StateFlow<List<DeviceItem>> = combine(repo.peers, repo.trusted) { peers, trusted ->
        val trustedIds = trusted.map { it.deviceId }.toSet()
        peers.filter { !it.trusted && it.deviceId !in trustedIds }
            .map { DeviceItem(it.deviceId, it.name, it.kind, trusted = false, online = true, path = it.bestPath) }
            .sortedBy { it.name.lowercase() }
    }.stateIn(viewModelScope, SharingStarted.Eagerly, emptyList())

    init {
        viewModelScope.launch {
            repo.events.collect { e ->
                when (e) {
                    is UiEvent.Linked -> _link.value = null
                    is UiEvent.Error -> _link.update { s -> if (s?.busy == true) s.copy(busy = false, error = e.text) else s }
                    is UiEvent.Info -> Unit
                }
            }
        }
    }

    // ----------------------------------------------------------- navigation

    fun selectTab(tab: Tab) {
        _tab.value = tab
    }

    fun show(screen: Screen) {
        _screen.value = screen
    }

    fun showQr(visible: Boolean) {
        _qrVisible.value = visible
    }

    // --------------------------------------------------------------- sending

    fun onDeviceClick(item: DeviceItem) {
        if (!item.online) return
        val target = SendTarget.Peer(item.id, item.name)
        if (!repo.sendStaged(target)) requestPick(target, PickKind.FILES)
    }

    fun requestPick(target: SendTarget, kind: PickKind) {
        pendingTarget = target
        picks.trySend(kind)
    }

    fun onFilesPicked(uris: List<Uri>) {
        val target = pendingTarget.also { pendingTarget = null } ?: return
        if (uris.isEmpty()) return
        prepareAndSend(target) { preparer.fromUris(uris) }
    }

    fun onFolderPicked(uri: Uri?) {
        val target = pendingTarget.also { pendingTarget = null } ?: return
        if (uri == null) return
        prepareAndSend(target) { preparer.fromTree(uri) }
    }

    private fun prepareAndSend(target: SendTarget, prepare: suspend () -> app.warpdrop.android.share.PreparedSend) {
        viewModelScope.launch {
            _preparing.value = true
            try {
                val prepared = prepare()
                // The picker may have outlived an idle service: wait for the node.
                repo.awaitNode()
                if (repo.sendFiles(target, prepared) != null && target is SendTarget.Code) clearCode()
            } catch (e: UnreadableItemException) {
                repo.emit(UiEvent.Error(e.text))
            } catch (e: Exception) {
                repo.emit(UiEvent.Error(EngineErrors.fromException(e, (target as? SendTarget.Peer)?.name)))
            } finally {
                _preparing.value = false
            }
        }
    }

    fun sendClipboard(target: SendTarget) {
        val content = try {
            ClipboardBridge.read(app)
        } catch (e: ClipboardBridge.TooLargeException) {
            repo.emit(UiEvent.Error(UiText.res(R.string.error_too_large)))
            return
        } catch (e: Exception) {
            repo.emit(UiEvent.Error(EngineErrors.fromException(e, null)))
            return
        }
        if (content == null) {
            repo.emit(UiEvent.Info(UiText.res(R.string.clipboard_empty)))
            return
        }
        if (repo.sendClipboard(target, content) != null && target is SendTarget.Code) clearCode()
    }

    fun untrust(item: DeviceItem) {
        repo.untrust(item.id)
    }

    fun clearStaged() = repo.clearStaged()

    // ---------------------------------------------------------- send to code

    fun onCodeInput(text: String) {
        if (text.contains("warpdrop:", ignoreCase = true)) {
            val raw = text.trim()
            val normalized = runCatching { normalizeCode(raw) }.getOrNull()
            if (normalized != null) {
                codePayload = raw
                _codeInput.value = normalized
                return
            }
        }
        codePayload = null
        _codeInput.value = text.filter { it.isDigit() }.take(6)
    }

    private fun clearCode() {
        codePayload = null
        _codeInput.value = ""
    }

    /** The current field as a target, once 6 digits are present. */
    fun codeTarget(): SendTarget.Code? {
        val digits = _codeInput.value
        if (digits.length != 6) return null
        return SendTarget.Code(codePayload ?: digits)
    }

    /** Staged items go straight out; otherwise the file picker opens. */
    fun submitCode() {
        val target = codeTarget() ?: return
        if (repo.sendStaged(target)) clearCode() else requestPick(target, PickKind.FILES)
    }

    /** From the QR scanner. Returns true when [raw] is a WarpDrop code/payload. */
    fun onScanned(raw: String): Boolean {
        val normalized = runCatching { normalizeCode(raw) }.getOrNull() ?: return false
        codePayload = raw.trim()
        _codeInput.value = normalized
        _screen.value = Screen.HOME
        submitCode()
        return true
    }

    // ------------------------------------------------------------ transfers

    fun pause(id: String) = repo.pause(id)
    fun resume(id: String) = repo.resume(id)
    fun cancel(id: String) = repo.cancel(id)
    fun clearFinished() = repo.clearFinished()
    fun respond(offerId: String, accept: Boolean) = repo.respondToOffer(offerId, accept)

    // --------------------------------------------------------------- linking

    fun openLink() {
        _link.value = LinkSheetState(mode = LinkMode.SHOW, windowExpiresAtMs = System.currentTimeMillis() + LINK_WINDOW_SECS * 1000L)
        repo.openLinkWindow(LINK_WINDOW_SECS.toUInt())
    }

    fun setLinkMode(mode: LinkMode) {
        _link.update { it?.copy(mode = mode, error = null) }
        if (mode == LinkMode.SHOW) {
            repo.openLinkWindow(LINK_WINDOW_SECS.toUInt())
            _link.update { it?.copy(windowExpiresAtMs = System.currentTimeMillis() + LINK_WINDOW_SECS * 1000L) }
        }
    }

    fun onLinkInput(text: String) {
        val digits = if (text.contains("warpdrop:", ignoreCase = true)) {
            runCatching { normalizeCode(text.trim()) }.getOrNull() ?: ""
        } else {
            text.filter { it.isDigit() }.take(6)
        }
        _link.update { it?.copy(input = digits, error = null) }
    }

    fun submitLink() {
        val s = _link.value ?: return
        if (s.input.length != 6 || s.busy) return
        _link.value = s.copy(busy = true, error = null)
        if (!repo.linkWithCode(s.input)) _link.update { it?.copy(busy = false) }
    }

    fun closeLink() {
        repo.closeLinkWindow()
        _link.value = null
    }

    // -------------------------------------------------------------- settings

    fun setDeviceName(name: String) = repo.setDeviceName(name)

    fun setRelayUrl(url: String) {
        repo.setRelayUrl(url)
    }

    fun relayUrl(): String = repo.settingsStore.relayUrl.orEmpty()

    fun language(): Locales.Choice = Locales.current(app)

    fun setLanguage(choice: Locales.Choice) = Locales.set(app, choice)

    companion object {
        const val LINK_WINDOW_SECS = 300
    }
}
