package app.warpdrop.android.ui.theme

import androidx.compose.animation.core.Spring
import androidx.compose.animation.core.spring
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.ColorScheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Shapes
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.em
import androidx.compose.ui.unit.sp

/** Design tokens from docs/design-system.md. */
@Immutable
data class WarpColors(
    val bg: Color,
    val surface: Color,
    val surfaceHover: Color,
    val border: Color,
    val text: Color,
    val textSecondary: Color,
    val textTertiary: Color,
    val accent: Color,
    val accentSoft: Color,
    val success: Color,
    val warning: Color,
    val danger: Color,
    val cable: Color,
)

val LightTokens = WarpColors(
    bg = Color(0xFFF6F6F7),
    surface = Color(0xFFFFFFFF),
    surfaceHover = Color(0xFFF1F1F3),
    border = Color(0x14000000), // rgba(0,0,0,0.08)
    text = Color(0xFF111113),
    textSecondary = Color(0xFF6B6B73),
    textTertiary = Color(0xFF9A9AA3),
    accent = Color(0xFF2F6BFF),
    accentSoft = Color(0x1A2F6BFF), // 0.10
    success = Color(0xFF1F9D55),
    warning = Color(0xFFC27C0E),
    danger = Color(0xFFD93D3D),
    cable = Color(0xFFC27C0E),
)

val DarkTokens = WarpColors(
    bg = Color(0xFF141416),
    surface = Color(0xFF1C1C1F),
    surfaceHover = Color(0xFF232327),
    border = Color(0x14FFFFFF), // rgba(255,255,255,0.08)
    text = Color(0xFFEDEDEF),
    textSecondary = Color(0xFF9A9AA3),
    textTertiary = Color(0xFF6B6B73),
    accent = Color(0xFF4C82FF),
    accentSoft = Color(0x294C82FF), // 0.16
    success = Color(0xFF34C26F),
    warning = Color(0xFFE0A030),
    danger = Color(0xFFF06262),
    cable = Color(0xFFE0A030),
)

val LocalWarpColors = staticCompositionLocalOf { LightTokens }

object Warp {
    val colors: WarpColors
        @Composable get() = LocalWarpColors.current

    val cardShape = RoundedCornerShape(12.dp)
    val controlShape = RoundedCornerShape(8.dp)
    val badgeShape = RoundedCornerShape(6.dp)

    /** Phone window padding. */
    val pagePadding = 16.dp

    /** spring(response 0.35 s, damping 0.8). */
    fun <T> spring() = spring<T>(dampingRatio = 0.8f, stiffness = Spring.StiffnessMediumLow)

    val codeStyle = TextStyle(
        fontFamily = FontFamily.Monospace,
        fontWeight = FontWeight.SemiBold,
        fontSize = 28.sp,
        letterSpacing = 0.04.em,
        fontFeatureSettings = "tnum",
    )
}

private fun scheme(t: WarpColors, dark: Boolean): ColorScheme {
    val base = if (dark) darkColorScheme() else lightColorScheme()
    return base.copy(
        primary = t.accent,
        onPrimary = Color.White,
        primaryContainer = t.accentSoft,
        onPrimaryContainer = t.accent,
        secondary = t.textSecondary,
        onSecondary = t.surface,
        secondaryContainer = t.surfaceHover,
        onSecondaryContainer = t.text,
        tertiary = t.success,
        background = t.bg,
        onBackground = t.text,
        surface = t.surface,
        onSurface = t.text,
        surfaceVariant = t.surfaceHover,
        onSurfaceVariant = t.textSecondary,
        surfaceTint = Color.Transparent,
        surfaceBright = t.surface,
        surfaceDim = t.bg,
        surfaceContainerLowest = t.surface,
        surfaceContainerLow = t.surface,
        surfaceContainer = t.surface,
        surfaceContainerHigh = t.surface,
        surfaceContainerHighest = t.surfaceHover,
        inverseSurface = t.text,
        inverseOnSurface = t.surface,
        outline = t.border,
        outlineVariant = t.border,
        error = t.danger,
        onError = Color.White,
        errorContainer = t.danger.copy(alpha = 0.12f),
        onErrorContainer = t.danger,
        scrim = Color.Black.copy(alpha = 0.32f),
    )
}

private val typography = Typography().let { d ->
    d.copy(
        // title 20/600
        titleLarge = d.titleLarge.copy(fontSize = 20.sp, lineHeight = 26.sp, fontWeight = FontWeight.SemiBold),
        titleMedium = d.titleMedium.copy(fontSize = 16.sp, lineHeight = 22.sp, fontWeight = FontWeight.SemiBold),
        // section 13/600
        titleSmall = d.titleSmall.copy(fontSize = 13.sp, lineHeight = 18.sp, fontWeight = FontWeight.SemiBold),
        // body 16/400 (mobile)
        bodyLarge = d.bodyLarge.copy(fontSize = 16.sp, lineHeight = 22.sp),
        bodyMedium = d.bodyMedium.copy(fontSize = 14.sp, lineHeight = 20.sp),
        // caption 12/500
        labelSmall = d.labelSmall.copy(fontSize = 12.sp, lineHeight = 16.sp, fontWeight = FontWeight.Medium, letterSpacing = 0.sp),
        labelMedium = d.labelMedium.copy(fontSize = 12.sp, lineHeight = 16.sp, fontWeight = FontWeight.Medium),
        labelLarge = d.labelLarge.copy(fontSize = 14.sp, lineHeight = 20.sp, fontWeight = FontWeight.SemiBold),
    )
}

private val shapes = Shapes(
    extraSmall = RoundedCornerShape(6.dp),
    small = RoundedCornerShape(8.dp),
    medium = RoundedCornerShape(12.dp),
    large = RoundedCornerShape(16.dp),
    extraLarge = RoundedCornerShape(20.dp),
)

@Composable
fun WarpTheme(dark: Boolean = isSystemInDarkTheme(), content: @Composable () -> Unit) {
    val tokens = if (dark) DarkTokens else LightTokens
    CompositionLocalProvider(LocalWarpColors provides tokens) {
        MaterialTheme(colorScheme = scheme(tokens, dark), typography = typography, shapes = shapes, content = content)
    }
}
