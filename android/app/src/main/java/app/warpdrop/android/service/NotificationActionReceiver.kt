package app.warpdrop.android.service

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import app.warpdrop.android.core.WarpRepository

/** Handles Cancel / Accept / Decline from notifications (same process as the node). */
class NotificationActionReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val id = intent.getStringExtra(EXTRA_ID)
        when (intent.action) {
            ACTION_CANCEL -> if (id != null) WarpRepository.cancel(id) else WarpRepository.cancelAllActive()
            ACTION_ACCEPT -> if (id != null) WarpRepository.respondToOffer(id, true)
            ACTION_DECLINE -> if (id != null) WarpRepository.respondToOffer(id, false)
        }
    }

    companion object {
        const val ACTION_CANCEL = "app.warpdrop.android.action.CANCEL"
        const val ACTION_ACCEPT = "app.warpdrop.android.action.ACCEPT"
        const val ACTION_DECLINE = "app.warpdrop.android.action.DECLINE"
        const val EXTRA_ID = "id"
    }
}
