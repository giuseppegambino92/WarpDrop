package app.warpdrop.android.tile

import android.annotation.SuppressLint
import android.app.PendingIntent
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.graphics.drawable.Icon
import android.os.Build
import android.service.quicksettings.Tile
import android.service.quicksettings.TileService
import app.warpdrop.android.R
import app.warpdrop.android.core.WarpRepository
import app.warpdrop.android.ui.MainActivity
import app.warpdrop.android.util.Locales
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch

/** Quick Settings tile: "WarpDrop — Code 849-201"; tap opens the Quick Code tab. */
class CodeTileService : TileService() {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private var job: Job? = null

    override fun onStartListening() {
        super.onStartListening()
        job?.cancel()
        job = scope.launch { WarpRepository.code.collect { render() } }
    }

    override fun onStopListening() {
        job?.cancel()
        job = null
        super.onStopListening()
    }

    override fun onDestroy() {
        scope.cancel()
        super.onDestroy()
    }

    private fun render() {
        val tile = qsTile ?: return
        val c = Locales.localized(this)
        val code = WarpRepository.code.value
        tile.label = c.getString(R.string.qs_tile_label)
        tile.subtitle = code?.let { c.getString(R.string.qs_tile_subtitle, it.display) }
        tile.contentDescription = listOfNotNull(tile.label, tile.subtitle).joinToString(", ")
        tile.state = if (code != null) Tile.STATE_ACTIVE else Tile.STATE_INACTIVE
        tile.icon = Icon.createWithResource(this, R.drawable.ic_stat_warpdrop)
        tile.updateTile()
    }

    override fun onClick() {
        super.onClick()
        // Opening the activity starts WarpService (foreground-start rules
        // require a visible activity) and shows the code + QR.
        val intent = Intent(this, MainActivity::class.java)
            .putExtra(MainActivity.EXTRA_TAB, MainActivity.TAB_QUICK_CODE)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP)
        val open = { launch(intent) }
        if (isLocked) unlockAndRun(open) else open()
    }

    @SuppressLint("StartActivityAndCollapseDeprecated")
    private fun launch(intent: Intent) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            val pi = PendingIntent.getActivity(this, 0, intent, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
            startActivityAndCollapse(pi)
        } else {
            @Suppress("DEPRECATION") // The Intent overload is the only one below API 34.
            startActivityAndCollapse(intent)
        }
    }

    companion object {
        fun requestUpdate(context: Context) {
            runCatching {
                requestListeningState(context, ComponentName(context, CodeTileService::class.java))
            }
        }
    }
}
