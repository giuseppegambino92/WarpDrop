package app.warpdrop.android.service

import android.app.Notification
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.util.Log
import androidx.core.app.NotificationManagerCompat
import androidx.core.app.ServiceCompat
import androidx.core.content.ContextCompat
import androidx.lifecycle.LifecycleService
import androidx.lifecycle.lifecycleScope
import app.warpdrop.android.AppVisibility
import app.warpdrop.android.core.NodeStatus
import app.warpdrop.android.core.WarpRepository
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.drop
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * Owns the process' single WarpNode while the app is open or a transfer runs.
 *
 * Always foreground (it is only ever started while an activity is visible):
 * type `connectedDevice` while idle and `dataSync|connectedDevice` while a
 * transfer runs. When the UI is gone and nothing is in flight it lingers for
 * [IDLE_GRACE_MS] (so a phone in your pocket still receives a quick send),
 * then shuts the node down and stops.
 */
class WarpService : LifecycleService() {
    private lateinit var nsd: NsdDiscovery
    private lateinit var network: NetworkMonitor
    private var currentType = 0
    private var lastNotify = 0L
    private var discoveryStarted = false

    override fun onCreate() {
        super.onCreate()
        nsd = NsdDiscovery(this)
        network = NetworkMonitor(this) {
            WarpRepository.refreshNetwork()
            if (discoveryStarted) lifecycleScope.launch(Dispatchers.IO) { nsd.restartDiscovery() }
        }
        promote(WarpRepository.hasActiveTransfers)

        lifecycleScope.launch(Dispatchers.IO) {
            WarpRepository.startNode(this@WarpService) ?: return@launch
            nsd.start()
            discoveryStarted = true
        }
        network.start()

        // Live notification + foreground type.
        lifecycleScope.launch {
            combine(WarpRepository.transfers, WarpRepository.code) { t, c -> t to c }.collect { (transfers, code) ->
                val active = transfers.filter { !WarpRepository.isTerminal(it.state) }
                val wantType = foregroundType(active.isNotEmpty())
                val notification = Notifications.serviceNotification(this@WarpService, code, active)
                if (wantType != currentType) {
                    promote(active.isNotEmpty(), notification)
                } else {
                    val now = System.currentTimeMillis()
                    // Progress updates at most ~1/s (the system rate-limits faster updates).
                    if (active.isEmpty() || now - lastNotify >= 1000) {
                        lastNotify = now
                        notify(notification)
                    }
                }
            }
        }

        // A device-name change alters the TXT record: re-advertise.
        lifecycleScope.launch {
            WarpRepository.identity.map { it?.name }.distinctUntilChanged().drop(1).collect {
                if (discoveryStarted) withContext(Dispatchers.IO) { nsd.reRegister() }
            }
        }

        // Stop when the UI is gone and nothing is pending.
        lifecycleScope.launch {
            combine(
                AppVisibility.foreground,
                WarpRepository.transfers.map { list -> list.any { !WarpRepository.isTerminal(it.state) } },
                WarpRepository.offers.map { it.isNotEmpty() },
                WarpRepository.status,
            ) { visible, busy, offers, status -> visible || busy || offers || status == NodeStatus.Starting }
                .distinctUntilChanged()
                .collectLatest { needed ->
                    if (!needed) {
                        delay(IDLE_GRACE_MS)
                        Log.i(TAG, "idle; stopping")
                        stopSelf()
                    }
                }
        }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        super.onStartCommand(intent, flags, startId)
        // Not sticky: restarting a foreground service from the background is not allowed.
        return START_NOT_STICKY
    }

    override fun onTaskRemoved(rootIntent: Intent?) {
        super.onTaskRemoved(rootIntent)
        if (!WarpRepository.hasActiveTransfers && WarpRepository.offers.value.isEmpty()) stopSelf()
    }

    /** Android 15+: the daily dataSync budget is exhausted. */
    override fun onTimeout(startId: Int, fgsType: Int) {
        Log.w(TAG, "foreground service timeout (type $fgsType)")
        WarpRepository.cancelAllActive()
        stopSelf()
    }

    override fun onDestroy() {
        network.stop()
        val discovery = nsd
        // Detach now (a new service instance must not reuse this node); the
        // blocking shutdown itself runs off the main thread.
        WarpRepository.stopNode(async = true) { discovery.stop() }
        ServiceCompat.stopForeground(this, ServiceCompat.STOP_FOREGROUND_REMOVE)
        super.onDestroy()
    }

    private fun foregroundType(transferring: Boolean): Int =
        if (transferring) {
            ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC or ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE
        } else {
            ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE
        }

    private fun promote(
        transferring: Boolean,
        notification: Notification = Notifications.serviceNotification(
            this,
            WarpRepository.code.value,
            WarpRepository.transfers.value.filter { !WarpRepository.isTerminal(it.state) },
        ),
    ) {
        val type = foregroundType(transferring)
        try {
            ServiceCompat.startForeground(this, Notifications.ID_SERVICE, notification, type)
            currentType = type
        } catch (e: Exception) {
            // e.g. ForegroundServiceStartNotAllowedException when switching type
            // from the background: keep the previous type, just refresh the text.
            Log.w(TAG, "startForeground(type=$type) failed", e)
            if (currentType == 0) {
                stopSelf()
            } else {
                notify(notification)
            }
        }
    }

    private fun notify(notification: Notification) {
        try {
            NotificationManagerCompat.from(this).notify(Notifications.ID_SERVICE, notification)
        } catch (e: SecurityException) {
            // POST_NOTIFICATIONS denied: the service keeps running without a visible card.
        }
    }

    companion object {
        private const val TAG = "WarpService"
        private const val IDLE_GRACE_MS = 3 * 60 * 1000L

        /** Call while an activity is visible (foreground-service start rules). */
        fun start(context: Context) {
            try {
                ContextCompat.startForegroundService(context, Intent(context, WarpService::class.java))
            } catch (e: Exception) {
                Log.w(TAG, "cannot start the service", e)
            }
        }
    }
}
