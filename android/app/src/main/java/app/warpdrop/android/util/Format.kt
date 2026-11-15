package app.warpdrop.android.util

import android.content.Context
import android.text.format.Formatter
import app.warpdrop.android.R
import uniffi.warp.DeviceKind
import uniffi.warp.PathKind
import java.util.Locale

object Format {
    fun bytes(context: Context, bytes: ULong): String =
        Formatter.formatShortFileSize(context, bytes.toLong().coerceAtLeast(0))

    /** MB/s (decimal), switching to Gbit/s above 1000 MB/s as the design system specifies. */
    fun speed(context: Context, bytesPerSec: ULong): String {
        val locale = context.resources.configuration.locales[0] ?: Locale.getDefault()
        val mbps = bytesPerSec.toDouble() / 1_000_000.0
        return when {
            mbps >= 1000.0 -> String.format(locale, "%.1f Gbit/s", bytesPerSec.toDouble() * 8.0 / 1_000_000_000.0)
            mbps >= 10.0 -> String.format(locale, "%.0f MB/s", mbps)
            else -> String.format(locale, "%.1f MB/s", mbps)
        }
    }

    fun eta(context: Context, secs: ULong): String {
        val s = secs.toLong()
        return if (s < 60) {
            context.getString(R.string.eta_seconds, s.toString())
        } else {
            context.getString(R.string.eta_minutes, ((s + 59) / 60).toString())
        }
    }

    /** `7:42` */
    fun countdown(millis: Long): String {
        val total = (millis.coerceAtLeast(0) + 999) / 1000
        return "%d:%02d".format(Locale.ROOT, total / 60, total % 60)
    }

    fun pathBadge(context: Context, path: PathKind): String = context.getString(
        when (path) {
            PathKind.USB_CABLE, PathKind.ADB_USB, PathKind.LOOPBACK -> R.string.badge_usb
            PathKind.THUNDERBOLT_BRIDGE -> R.string.badge_thunderbolt
            PathKind.WIFI_DIRECT -> R.string.badge_wifi_direct
            PathKind.ETHERNET -> R.string.badge_ethernet
            PathKind.WIFI -> R.string.badge_wifi
            PathKind.LAN -> R.string.badge_lan
            PathKind.INTERNET_P2P -> R.string.badge_p2p
            PathKind.RELAY -> R.string.badge_relay
        },
    )

    fun isCable(path: PathKind?): Boolean =
        path == PathKind.USB_CABLE || path == PathKind.ADB_USB || path == PathKind.THUNDERBOLT_BRIDGE

    fun shortcutIcon(kind: DeviceKind): Int = when (kind) {
        DeviceKind.MAC -> R.drawable.ic_shortcut_laptop
        DeviceKind.I_PHONE, DeviceKind.ANDROID -> R.drawable.ic_shortcut_phone
        DeviceKind.I_PAD -> R.drawable.ic_shortcut_tablet
        DeviceKind.WINDOWS, DeviceKind.LINUX, DeviceKind.UNKNOWN -> R.drawable.ic_shortcut_desktop
    }
}
