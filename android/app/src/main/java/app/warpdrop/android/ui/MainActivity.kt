package app.warpdrop.android.ui

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.activity.viewModels
import androidx.appcompat.app.AppCompatActivity
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.material3.SnackbarHostState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.core.content.ContextCompat
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LifecycleEventEffect
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import app.warpdrop.android.R
import app.warpdrop.android.core.UiEvent
import app.warpdrop.android.core.WarpRepository
import app.warpdrop.android.service.Notifications
import app.warpdrop.android.service.WarpService
import app.warpdrop.android.ui.components.LinkSheet
import app.warpdrop.android.ui.components.OfferDialog
import app.warpdrop.android.ui.components.QrDialog
import app.warpdrop.android.ui.screens.HomeScreen
import app.warpdrop.android.ui.screens.Notice
import app.warpdrop.android.ui.screens.ScannerScreen
import app.warpdrop.android.ui.screens.SettingsScreen
import app.warpdrop.android.ui.theme.Warp
import app.warpdrop.android.ui.theme.WarpTheme

class MainActivity : AppCompatActivity() {
    private val vm: MainViewModel by viewModels()

    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        // Channel names follow the (possibly just changed) in-app language.
        Notifications.createChannels(this)
        if (savedInstanceState == null) handleIntent(intent)
        setContent { WarpTheme { WarpRoot(vm) } }
    }

    override fun onStart() {
        super.onStart()
        // The node lives in a foreground service; start it while we are visible.
        WarpService.start(this)
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        handleIntent(intent)
    }

    private fun handleIntent(intent: Intent) {
        when (intent.getStringExtra(EXTRA_TAB)) {
            TAB_QUICK_CODE -> vm.selectTab(Tab.QUICK_CODE)
            TAB_MY_DEVICES -> vm.selectTab(Tab.MY_DEVICES)
        }
        val data = intent.data
        if (intent.action == Intent.ACTION_VIEW && data?.scheme == "warpdrop") {
            vm.onCodeInput(data.toString())
            vm.selectTab(Tab.QUICK_CODE)
            vm.show(Screen.HOME)
        }
        if (WarpRepository.staged.value != null) vm.show(Screen.HOME)
    }

    companion object {
        const val EXTRA_TAB = "app.warpdrop.android.extra.TAB"
        const val TAB_QUICK_CODE = "quick_code"
        const val TAB_MY_DEVICES = "my_devices"
    }
}

private fun granted(context: android.content.Context, permission: String) =
    ContextCompat.checkSelfPermission(context, permission) == PackageManager.PERMISSION_GRANTED

/** Permissions WarpDrop needs at runtime on this OS version, with their rationale. */
private fun runtimePermissions(): List<Pair<String, Int>> = buildList {
    if (Build.VERSION.SDK_INT >= 37) add(LOCAL_NETWORK to R.string.permission_local_network)
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) add(Manifest.permission.POST_NOTIFICATIONS to R.string.permission_notifications)
}

/** `Manifest.permission.ACCESS_LOCAL_NETWORK` (API 37). */
private const val LOCAL_NETWORK = "android.permission.ACCESS_LOCAL_NETWORK"

@Composable
private fun WarpRoot(vm: MainViewModel) {
    val context = LocalContext.current
    val snackbar = remember { SnackbarHostState() }
    val screen by vm.screen.collectAsStateWithLifecycle()
    val offers by vm.offers.collectAsStateWithLifecycle()
    val link by vm.link.collectAsStateWithLifecycle()
    val code by vm.code.collectAsStateWithLifecycle()
    val qrVisible by vm.qrVisible.collectAsStateWithLifecycle()

    // System pickers. Files: OpenMultipleDocuments; folders: OpenDocumentTree.
    val pickFiles = rememberLauncherForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { vm.onFilesPicked(it) }
    val pickFolder = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocumentTree()) { vm.onFolderPicked(it) }
    LaunchedEffect(Unit) {
        vm.pickRequests.collect { kind ->
            when (kind) {
                PickKind.FILES -> pickFiles.launch(arrayOf("*/*"))
                PickKind.FOLDER -> pickFolder.launch(null)
            }
        }
    }

    // Snackbars for engine errors and confirmations.
    LaunchedEffect(Unit) {
        vm.events.collect { e ->
            val text = when (e) {
                is UiEvent.Error -> e.text.resolve(context)
                is UiEvent.Info -> e.text.resolve(context)
                is UiEvent.Linked -> null
            }
            if (text != null) snackbar.showSnackbar(text)
        }
    }

    // Runtime permissions: ask once up front, then offer banners.
    var permissionTick by remember { mutableIntStateOf(0) }
    val permissionLauncher = rememberLauncherForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) {
        permissionTick++
    }
    LaunchedEffect(Unit) {
        val settings = WarpRepository.settingsStore
        val missing = runtimePermissions().map { it.first }.filterNot { granted(context, it) }
        if (!settings.askedForPermissions && missing.isNotEmpty()) {
            settings.askedForPermissions = true
            permissionLauncher.launch(missing.toTypedArray())
        }
    }
    LifecycleEventEffect(Lifecycle.Event.ON_RESUME) { permissionTick++ }
    val missing = remember(permissionTick) { runtimePermissions().filterNot { granted(context, it.first) } }
    val allow = stringResource(R.string.allow)
    val accent = Warp.colors.accent
    val banners: LazyListScope.() -> Unit = {
        missing.forEach { (permission, rationale) ->
            item(key = "perm:$permission") {
                Notice(stringResource(rationale), tint = accent, action = allow) {
                    permissionLauncher.launch(arrayOf(permission))
                }
            }
        }
    }

    AnimatedContent(
        targetState = screen,
        transitionSpec = { fadeIn() togetherWith fadeOut() },
        label = "screen",
    ) { target ->
        when (target) {
            Screen.HOME -> HomeScreen(vm, snackbar, banners)
            Screen.SETTINGS -> SettingsScreen(vm)
            Screen.SCANNER -> ScannerScreen(onResult = vm::onScanned, onClose = { vm.show(Screen.HOME) })
        }
    }

    offers.firstOrNull()?.let { offer ->
        OfferDialog(
            offer = offer,
            onAccept = { vm.respond(offer.offerId, true) },
            onDecline = { vm.respond(offer.offerId, false) },
        )
    }
    link?.let { state ->
        LinkSheet(
            state = state,
            code = code,
            onMode = vm::setLinkMode,
            onInput = vm::onLinkInput,
            onSubmit = vm::submitLink,
            onDismiss = vm::closeLink,
        )
    }
    val qrCode = code
    if (qrVisible && qrCode != null) QrDialog(qrCode, onDismiss = { vm.showQr(false) })
}
