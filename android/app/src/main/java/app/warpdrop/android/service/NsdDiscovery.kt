package app.warpdrop.android.service

import android.content.Context
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.net.wifi.WifiManager
import android.os.Build
import android.util.Log
import androidx.annotation.RequiresApi
import app.warpdrop.android.core.WarpRepository
import uniffi.warp.PeerAdvert
import java.net.Inet6Address
import java.net.InetAddress
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.Executor
import java.util.concurrent.Executors

/**
 * Platform-native discovery (the core's own mDNS is disabled on Android):
 * registers `_warpdrop._udp` with the core's TXT record, browses for the same
 * type, resolves each instance and feeds it to `WarpNode.injectPeer`.
 * Holds a Wi-Fi MulticastLock while running, which the core's UDP broadcast
 * beacon needs as well.
 */
class NsdDiscovery(context: Context) {
    private val app = context.applicationContext
    private val nsd = app.getSystemService(NsdManager::class.java)
    private val wifi = app.getSystemService(WifiManager::class.java)
    private val executor: Executor = Executors.newSingleThreadExecutor { r -> Thread(r, "warp-nsd") }

    private var multicastLock: WifiManager.MulticastLock? = null
    private var registration: NsdManager.RegistrationListener? = null
    private var registeredName: String? = null
    private var discovery: NsdManager.DiscoveryListener? = null
    private var browseType = SERVICE_TYPE

    /** serviceName → device id we injected (to remove it on loss). */
    private val injected = ConcurrentHashMap<String, String>()

    /** API 34+: live resolution callbacks per service name. */
    private val infoCallbacks = ConcurrentHashMap<String, Any>()

    /** API 30–33: `resolveService` handles one request at a time. */
    private val resolveQueue = ArrayDeque<NsdServiceInfo>()
    private var resolving = false

    @Synchronized
    fun start() {
        acquireMulticast()
        register()
        discover()
    }

    @Synchronized
    fun stop() {
        stopDiscovery()
        unregister()
        releaseMulticast()
    }

    /** Re-advertise (e.g. after a device-name change: the TXT record changed). */
    @Synchronized
    fun reRegister() {
        unregister()
        register()
    }

    /** Network changed: restart browsing so stale/added links are re-resolved. */
    @Synchronized
    fun restartDiscovery() {
        stopDiscovery()
        discover()
        reRegister()
    }

    private fun acquireMulticast() {
        if (multicastLock?.isHeld == true) return
        multicastLock = wifi?.createMulticastLock("warpdrop")?.apply {
            setReferenceCounted(false)
            runCatching { acquire() }.onFailure { Log.w(TAG, "multicast lock", it) }
        }
    }

    private fun releaseMulticast() {
        multicastLock?.let { if (it.isHeld) runCatching { it.release() } }
        multicastLock = null
    }

    // -------------------------------------------------------- registration

    private fun register() {
        if (registration != null) return
        val advert = WarpRepository.advertisement() ?: return
        browseType = advert.serviceType.trimEnd('.')
        val info = NsdServiceInfo().apply {
            serviceName = advert.instanceName
            serviceType = advert.serviceType.trimEnd('.')
            port = advert.port.toInt()
            // All TXT keys go through unchanged (v, id, n, k, pk, c, t = TCP carriage, …).
            advert.txt.forEach { (k, v) ->
                runCatching { setAttribute(k, v) }.onFailure { Log.w(TAG, "TXT attribute '$k' rejected", it) }
            }
        }
        val listener = object : NsdManager.RegistrationListener {
            override fun onServiceRegistered(info: NsdServiceInfo) {
                registeredName = info.serviceName
            }

            override fun onRegistrationFailed(info: NsdServiceInfo, errorCode: Int) {
                Log.w(TAG, "registration failed: $errorCode")
                synchronized(this@NsdDiscovery) { if (registration === this) registration = null }
            }

            override fun onServiceUnregistered(info: NsdServiceInfo) = Unit
            override fun onUnregistrationFailed(info: NsdServiceInfo, errorCode: Int) = Unit
        }
        registration = listener
        runCatching { nsd.registerService(info, NsdManager.PROTOCOL_DNS_SD, listener) }
            .onFailure { Log.w(TAG, "registerService", it); registration = null }
    }

    private fun unregister() {
        registration?.let { runCatching { nsd.unregisterService(it) } }
        registration = null
        registeredName = null
    }

    // ----------------------------------------------------------- discovery

    private fun discover() {
        if (discovery != null) return
        val listener = object : NsdManager.DiscoveryListener {
            override fun onDiscoveryStarted(serviceType: String) = Unit
            override fun onDiscoveryStopped(serviceType: String) = Unit

            override fun onStartDiscoveryFailed(serviceType: String, errorCode: Int) {
                Log.w(TAG, "discovery failed: $errorCode")
                synchronized(this@NsdDiscovery) { if (discovery === this) discovery = null }
            }

            override fun onStopDiscoveryFailed(serviceType: String, errorCode: Int) = Unit

            override fun onServiceFound(info: NsdServiceInfo) {
                if (info.serviceName == registeredName) return
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) watch(info) else enqueueResolve(info)
            }

            override fun onServiceLost(info: NsdServiceInfo) = lost(info.serviceName)
        }
        discovery = listener
        runCatching { nsd.discoverServices(browseType, NsdManager.PROTOCOL_DNS_SD, listener) }
            .onFailure { Log.w(TAG, "discoverServices", it); discovery = null }
    }

    private fun stopDiscovery() {
        discovery?.let { runCatching { nsd.stopServiceDiscovery(it) } }
        discovery = null
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            infoCallbacks.keys.toList().forEach(::unwatch)
        }
        synchronized(resolveQueue) {
            resolveQueue.clear()
        }
        injected.forEach { (_, deviceId) -> WarpRepository.removeInjectedPeer(deviceId) }
        injected.clear()
    }

    private fun lost(serviceName: String) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) unwatch(serviceName)
        injected.remove(serviceName)?.let(WarpRepository::removeInjectedPeer)
    }

    private fun inject(info: NsdServiceInfo, hosts: List<InetAddress>) {
        val txt = info.attributes.mapNotNull { (k, v) -> v?.let { k to String(it, Charsets.UTF_8) } }.toMap()
        if (txt.isEmpty() || hosts.isEmpty() || info.port <= 0) return
        val addresses = hosts.distinct().map { formatAddress(it, info.port) }
        WarpRepository.injectPeer(PeerAdvert(txt = txt, addresses = addresses))
        val deviceId = txt["id"] ?: info.serviceName
        injected[info.serviceName] = deviceId
    }

    // API 34+: registerServiceInfoCallback keeps addresses up to date.
    @RequiresApi(Build.VERSION_CODES.UPSIDE_DOWN_CAKE)
    private fun watch(info: NsdServiceInfo) {
        val name = info.serviceName
        if (infoCallbacks.containsKey(name)) return
        val cb = object : NsdManager.ServiceInfoCallback {
            override fun onServiceInfoCallbackRegistrationFailed(errorCode: Int) {
                infoCallbacks.remove(name)
            }

            override fun onServiceUpdated(serviceInfo: NsdServiceInfo) = inject(serviceInfo, serviceInfo.hostAddresses)
            override fun onServiceLost() {
                injected.remove(name)?.let(WarpRepository::removeInjectedPeer)
            }

            override fun onServiceInfoCallbackUnregistered() = Unit
        }
        infoCallbacks[name] = cb
        runCatching { nsd.registerServiceInfoCallback(info, executor, cb) }
            .onFailure { infoCallbacks.remove(name) }
    }

    @RequiresApi(Build.VERSION_CODES.UPSIDE_DOWN_CAKE)
    private fun unwatch(serviceName: String) {
        val cb = infoCallbacks.remove(serviceName) as? NsdManager.ServiceInfoCallback ?: return
        runCatching { nsd.unregisterServiceInfoCallback(cb) }
    }

    // API 30–33: serialized resolveService.
    private fun enqueueResolve(info: NsdServiceInfo) {
        synchronized(resolveQueue) {
            if (resolveQueue.none { it.serviceName == info.serviceName }) resolveQueue.addLast(info)
            if (resolving) return
            resolving = true
        }
        resolveNext()
    }

    private fun resolveNext() {
        val next = synchronized(resolveQueue) {
            resolveQueue.removeFirstOrNull().also { if (it == null) resolving = false }
        } ?: return
        @Suppress("DEPRECATION") // resolveService is the only option below API 34.
        nsd.resolveService(
            next,
            object : NsdManager.ResolveListener {
                override fun onResolveFailed(serviceInfo: NsdServiceInfo, errorCode: Int) {
                    Log.i(TAG, "resolve ${serviceInfo.serviceName} failed: $errorCode")
                    resolveNext()
                }

                override fun onServiceResolved(serviceInfo: NsdServiceInfo) {
                    @Suppress("DEPRECATION") // getHost() is the pre-34 API.
                    val host = serviceInfo.host
                    if (host != null) inject(serviceInfo, listOf(host))
                    resolveNext()
                }
            },
        )
    }

    companion object {
        private const val TAG = "NsdDiscovery"
        const val SERVICE_TYPE = "_warpdrop._udp"

        fun formatAddress(addr: InetAddress, port: Int): String {
            val host = addr.hostAddress ?: return ""
            return if (addr is Inet6Address) "[$host]:$port" else "$host:$port"
        }
    }
}
