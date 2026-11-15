package io.atomik.kb.ui.theme

import android.os.Build
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.ColorScheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.dynamicDarkColorScheme
import androidx.compose.material3.dynamicLightColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext

private val LightColors = lightColorScheme(
    primary = Color(0xFF1B6B66),
    onPrimary = Color.White,
    primaryContainer = Color(0xFFA6F2EB),
    onPrimaryContainer = Color(0xFF00201E),
    secondary = Color(0xFF4A6361),
    background = Color(0xFFF8FAF9),
    surface = Color(0xFFF8FAF9),
)

private val DarkColors = darkColorScheme(
    primary = Color(0xFF8AD5CF),
    onPrimary = Color(0xFF003734),
    primaryContainer = Color(0xFF00504C),
    onPrimaryContainer = Color(0xFFA6F2EB),
    secondary = Color(0xFFB0CCCA),
    background = Color(0xFF0F1514),
    surface = Color(0xFF0F1514),
)

@Composable
fun KbTheme(
    darkTheme: Boolean = isSystemInDarkTheme(),
    dynamicColor: Boolean = true,
    content: @Composable () -> Unit,
) {
    val colors: ColorScheme = when {
        dynamicColor && Build.VERSION.SDK_INT >= Build.VERSION_CODES.S -> {
            val context = LocalContext.current
            if (darkTheme) dynamicDarkColorScheme(context) else dynamicLightColorScheme(context)
        }
        darkTheme -> DarkColors
        else -> LightColors
    }
    MaterialTheme(colorScheme = colors, content = content)
}
