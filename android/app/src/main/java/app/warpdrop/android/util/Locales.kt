package app.warpdrop.android.util

import android.content.Context
import android.content.res.Configuration
import android.os.Build
import android.os.LocaleList
import androidx.appcompat.app.AppCompatDelegate
import androidx.core.content.edit
import androidx.core.os.LocaleListCompat

/**
 * Per-app language. Android 13+ applies it process-wide through LocaleManager
 * (AppCompat delegates to it); on Android 11–12 AppCompat only localizes its
 * activities, so services/notifications wrap their context with [localized].
 */
object Locales {
    enum class Choice(val tag: String?) { SYSTEM(null), ENGLISH("en"), ITALIAN("it") }

    private const val PREFS = "locale"
    private const val KEY = "tag"

    fun current(context: Context): Choice {
        val tags = AppCompatDelegate.getApplicationLocales().toLanguageTags()
            .ifEmpty { prefs(context).getString(KEY, "").orEmpty() }
        return when {
            tags.startsWith("it") -> Choice.ITALIAN
            tags.startsWith("en") -> Choice.ENGLISH
            else -> Choice.SYSTEM
        }
    }

    /** Applies immediately: AppCompat recreates visible activities. */
    fun set(context: Context, choice: Choice) {
        prefs(context).edit { putString(KEY, choice.tag.orEmpty()) }
        AppCompatDelegate.setApplicationLocales(
            choice.tag?.let { LocaleListCompat.forLanguageTags(it) } ?: LocaleListCompat.getEmptyLocaleList(),
        )
    }

    /** A context whose resources follow the in-app language choice. */
    fun localized(context: Context): Context {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) return context
        val tag = prefs(context).getString(KEY, "").orEmpty()
        if (tag.isEmpty()) return context
        val cfg = Configuration(context.resources.configuration)
        cfg.setLocales(LocaleList.forLanguageTags(tag))
        return context.createConfigurationContext(cfg)
    }

    private fun prefs(context: Context) = context.applicationContext.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
}
