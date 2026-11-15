package io.atomik.kb

import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.test.ext.junit.runners.AndroidJUnit4
import io.atomik.kb.engine.model.PageInfo
import io.atomik.kb.engine.model.SpaceInfo
import io.atomik.kb.features.home.HomeContent
import io.atomik.kb.features.home.HomeUiState
import io.atomik.kb.features.home.TreeRow
import io.atomik.kb.ui.theme.KbTheme
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class HomeContentTest {
    @get:Rule val compose = createComposeRule()

    @Test fun rowsAreShownAndTappingOpensThePage() {
        val opened = mutableListOf<String>()
        compose.setContent {
            KbTheme(dynamicColor = false) {
                HomeContent(
                    state = HomeUiState(
                        loading = false,
                        spaces = listOf(SpaceInfo("s1", "Personal")),
                        selectedSpace = SpaceInfo("s1", "Personal"),
                        rows = listOf(TreeRow(PageInfo("p1", "s1", null, "Shopping list"), 0, false, false)),
                    ),
                    onSelectSpace = {}, onToggleExpanded = {}, onOpenPage = { opened += it }, onNewPage = {},
                    onSearch = {}, onSettings = {}, onRefresh = {}, onRetry = {}, onMessageShown = {},
                )
            }
        }
        compose.onNodeWithText("Personal").assertIsDisplayed()
        compose.onNodeWithText("Shopping list").assertIsDisplayed().performClick()
        assertEquals(listOf("p1"), opened)
    }
}
