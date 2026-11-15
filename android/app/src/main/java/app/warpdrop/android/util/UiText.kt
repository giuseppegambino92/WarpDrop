package app.warpdrop.android.util

import android.content.Context
import androidx.annotation.StringRes
import app.warpdrop.android.R
import uniffi.warp.WarpException

/** Text resolved at display time, so it always follows the current UI language. */
sealed interface UiText {
    data class Res(@param:StringRes val id: Int, val args: List<Any> = emptyList()) : UiText
    data class Raw(val text: String) : UiText

    fun resolve(context: Context): String = when (this) {
        is Raw -> text
        is Res -> {
            val resolved = args.map { if (it is UiText) it.resolve(context) else it }
            if (resolved.isEmpty()) context.getString(id) else context.getString(id, *resolved.toTypedArray())
        }
    }

    companion object {
        fun res(@StringRes id: Int, vararg args: Any): UiText = Res(id, args.toList())
    }
}

/**
 * Maps engine errors (English `Display` strings from `WarpError`, or thrown
 * `WarpException`s) to the localized `error_*` strings.
 */
object EngineErrors {
    /** [peer] is a display name; null/blank falls back to "the other device". */
    fun fromMessage(message: String?, peer: String?): UiText {
        val who: Any = peer?.takeIf { it.isNotBlank() } ?: UiText.res(R.string.peer_unknown)
        val m = message.orEmpty().trim()
        val lower = m.lowercase()
        return when {
            lower.isEmpty() -> UiText.res(R.string.state_failed)
            lower.contains("wrong code") -> UiText.res(R.string.error_wrong_code)
            lower.contains("invalid code") -> UiText.res(R.string.error_wrong_code)
            lower.contains("no device is currently showing") -> UiText.res(R.string.error_code_not_found)
            lower.contains("rejected by the other device") && lower.contains("too_large") -> UiText.res(R.string.error_too_large)
            lower.contains("rejected by the other device") || lower.contains("declined") -> UiText.res(R.string.error_declined, who)
            lower.startsWith("unreachable") || lower.contains("timed out") || lower.contains("device not found") ||
                lower.contains("not nearby") || lower.contains("connection lost") -> UiText.res(R.string.error_unreachable, who)
            lower == "cancelled" -> UiText.res(R.string.state_cancelled)
            else -> UiText.res(R.string.error_generic, m)
        }
    }

    /** Stable `TransferSnapshot.errorCode`s from the core; [message] fills `error_generic`. */
    fun fromCode(code: String?, message: String?, peer: String?): UiText {
        val who: Any = peer?.takeIf { it.isNotBlank() } ?: UiText.res(R.string.peer_unknown)
        val detail = message?.takeIf { it.isNotBlank() } ?: code.orEmpty()
        return when (code) {
            null -> fromMessage(message, peer)
            "wrong_code" -> UiText.res(R.string.error_wrong_code)
            "code_not_found" -> UiText.res(R.string.error_code_not_found)
            "declined" -> UiText.res(R.string.error_declined, who)
            "unreachable", "timeout" -> UiText.res(R.string.error_unreachable, who)
            "cancelled" -> UiText.res(R.string.state_cancelled)
            "rejected" -> if (detail.contains("too_large")) UiText.res(R.string.error_too_large) else UiText.res(R.string.error_generic, detail)
            else -> UiText.res(R.string.error_generic, detail) // io, integrity, protocol, future codes
        }
    }

    fun fromException(e: Throwable, peer: String?): UiText {
        val who: Any = peer?.takeIf { it.isNotBlank() } ?: UiText.res(R.string.peer_unknown)
        return when (e) {
            is WarpException.WrongCode, is WarpException.InvalidCode -> UiText.res(R.string.error_wrong_code)
            is WarpException.CodeNotFound -> UiText.res(R.string.error_code_not_found)
            is WarpException.PeerNotFound, is WarpException.Timeout, is WarpException.Unreachable ->
                UiText.res(R.string.error_unreachable, who)
            is WarpException.Rejected -> fromMessage("rejected by the other device: ${e.reason}", peer)
            is WarpException.Cancelled -> UiText.res(R.string.state_cancelled)
            is WarpException.Io -> UiText.res(R.string.error_generic, e.msg)
            is WarpException.Crypto -> UiText.res(R.string.error_generic, e.msg)
            is WarpException.Protocol -> UiText.res(R.string.error_generic, e.msg)
            is WarpException.Config -> UiText.res(R.string.error_generic, e.msg)
            else -> UiText.res(R.string.error_generic, e.message ?: e.javaClass.simpleName)
        }
    }
}
