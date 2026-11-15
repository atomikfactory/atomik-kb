package io.atomik.kb.ui.nav

import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.Modifier
import androidx.navigation.compose.NavHost
import androidx.navigation.compose.composable
import androidx.navigation.compose.rememberNavController
import io.atomik.kb.data.PageOpener
import io.atomik.kb.features.capture.CaptureScreen
import io.atomik.kb.features.home.HomeRoute
import io.atomik.kb.features.search.SearchScreen
import io.atomik.kb.features.settings.SettingsScreen
import io.atomik.kb.features.share.ShareScreen
import kotlinx.coroutines.flow.Flow
import kotlinx.serialization.Serializable

/** Type-safe destinations. Pages are not destinations: the editor lives in [HomeRoute]'s detail pane. */
@Serializable data object Home
@Serializable data object Search
@Serializable data object Settings
@Serializable data object Capture
@Serializable data object Share

/** Things other apps and shortcuts ask the app to do. */
enum class NavRequest { Capture, Share }

@Composable
fun AppNavHost(
    opener: PageOpener,
    navRequests: Flow<NavRequest>,
    modifier: Modifier = Modifier,
) {
    val nav = rememberNavController()
    LaunchedEffect(navRequests) {
        navRequests.collect { request ->
            nav.navigate(if (request == NavRequest.Capture) Capture else Share) { launchSingleTop = true }
        }
    }
    NavHost(navController = nav, startDestination = Home, modifier = modifier) {
        composable<Home> {
            HomeRoute(
                openRequests = opener.requests,
                onSearch = { nav.navigate(Search) { launchSingleTop = true } },
                onSettings = { nav.navigate(Settings) { launchSingleTop = true } },
            )
        }
        composable<Search> { SearchScreen(onClose = { nav.popBackStack() }) }
        composable<Settings> { SettingsScreen(onBack = { nav.popBackStack() }) }
        composable<Capture> { CaptureScreen(onDone = { nav.popBackStack() }) }
        composable<Share> { ShareScreen(onClose = { nav.popBackStack() }) }
    }
}
