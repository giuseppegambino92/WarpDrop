package app.warpdrop.android.service

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.net.ConnectivityManager
import android.net.LinkProperties
import android.net.Network
import android.net.NetworkCapabilities
import android.net.NetworkRequest
import android.os.Handler
import android.os.Looper
import androidx.core.content.ContextCompat

/**
 * Reports link changes (Wi-Fi, Ethernet, USB networks, USB tethering) so the
 * UI and discovery refresh promptly when a cable path appears or goes away.
 */
class NetworkMonitor(context: Context, private val onChange: () -> Unit) {
    private val app = context.applicationContext
    private val cm = app.getSystemService(ConnectivityManager::class.java)
    private val handler = Handler(Looper.getMainLooper())
    private val fire = Runnable { onChange() }
    private var registered = false

    private val callback = object : ConnectivityManager.NetworkCallback() {
        override fun onAvailable(network: Network) = schedule()
        override fun onLost(network: Network) = schedule()
        override fun onLinkPropertiesChanged(network: Network, linkProperties: LinkProperties) = schedule()
        override fun onCapabilitiesChanged(network: Network, networkCapabilities: NetworkCapabilities) {
            if (networkCapabilities.hasTransport(NetworkCapabilities.TRANSPORT_USB) ||
                networkCapabilities.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET)
            ) {
                schedule()
            }
        }
    }

    /**
     * USB tethering (RNDIS/NCM, 192.168.42.x) is not a "network" of this
     * device, so it never reaches the callback above; these sticky system
     * broadcasts announce it instead.
     */
    private val usbReceiver = object : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) = schedule()
    }

    fun start() {
        if (registered) return
        registered = true
        // No INTERNET requirement: USB / Ethernet / local-only Wi-Fi count too.
        val request = NetworkRequest.Builder()
            .removeCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)
            .removeCapability(NetworkCapabilities.NET_CAPABILITY_NOT_VPN)
            .build()
        runCatching { cm?.registerNetworkCallback(request, callback) }
        val filter = IntentFilter().apply {
            addAction(ACTION_TETHER_STATE_CHANGED)
            addAction(ACTION_USB_STATE)
        }
        ContextCompat.registerReceiver(app, usbReceiver, filter, ContextCompat.RECEIVER_NOT_EXPORTED)
    }

    fun stop() {
        if (!registered) return
        registered = false
        runCatching { cm?.unregisterNetworkCallback(callback) }
        runCatching { app.unregisterReceiver(usbReceiver) }
        handler.removeCallbacks(fire)
    }

    private fun schedule() {
        handler.removeCallbacks(fire)
        handler.postDelayed(fire, DEBOUNCE_MS)
    }

    private companion object {
        const val DEBOUNCE_MS = 750L

        // Hidden framework actions (stable strings, sticky broadcasts).
        const val ACTION_TETHER_STATE_CHANGED = "android.net.conn.TETHER_STATE_CHANGED"
        const val ACTION_USB_STATE = "android.hardware.usb.action.USB_STATE"
    }
}
