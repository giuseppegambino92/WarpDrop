package app.warpdrop.android

import android.app.Application
import androidx.lifecycle.DefaultLifecycleObserver
import androidx.lifecycle.LifecycleOwner
import androidx.lifecycle.ProcessLifecycleOwner
import app.warpdrop.android.core.WarpRepository
import app.warpdrop.android.service.Notifications
import app.warpdrop.android.share.SendPreparer
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

class WarpApp : Application() {
    override fun onCreate() {
        super.onCreate()
        Notifications.createChannels(this)
        WarpRepository.init(this)
        // Leftovers from a previous process (copied folders, non-seekable streams).
        SendPreparer.cleanStaleOutgoing(this)
        ProcessLifecycleOwner.get().lifecycle.addObserver(AppVisibility)
    }
}

/** Whether any WarpDrop activity is visible (drives notifications vs. in-app UI). */
object AppVisibility : DefaultLifecycleObserver {
    private val _foreground = MutableStateFlow(false)
    val foreground: StateFlow<Boolean> = _foreground.asStateFlow()
    val isForeground: Boolean get() = _foreground.value

    override fun onStart(owner: LifecycleOwner) {
        _foreground.value = true
    }

    override fun onStop(owner: LifecycleOwner) {
        _foreground.value = false
    }
}
