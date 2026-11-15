package io.atomik.kb.features.onboarding

import io.atomik.kb.MainDispatcherRule
import io.atomik.kb.RootUiState
import io.atomik.kb.RootViewModel
import io.atomik.kb.TestEnv
import io.atomik.kb.engine.EngineState
import io.atomik.kb.engine.model.PairingState
import io.atomik.kb.engine.spaces
import kotlinx.coroutines.flow.launchIn
import kotlinx.coroutines.plus
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.flow.onEach
import kotlinx.coroutines.test.runTest
import org.junit.Rule
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlin.test.assertTrue

class OnboardingViewModelTest {
    @get:Rule val main = MainDispatcherRule()

    @Test fun invalidServerUrlIsRejectedBeforeAnyRequest() = runTest {
        val env = TestEnv(seed = false)
        val vm = OnboardingViewModel(env.spaces, env.sync)
        vm.goTo(OnboardingStep.Server)
        vm.onServerUrlChange("not a url")
        vm.startPairing()
        assertTrue(vm.state.value.serverUrlInvalid)
        assertEquals(OnboardingStep.Server, vm.state.value.step)
    }

    @Test fun joinerPairingShowsCodeThenDigitsThenFinishes() = runTest {
        val env = TestEnv(seed = false)
        val vm = OnboardingViewModel(env.spaces, env.sync)
        val root = RootViewModel(env.security, env.spaces)
        root.state.onEach { }.launchIn(backgroundScope + UnconfinedTestDispatcher(testScheduler))
        assertEquals(RootUiState.Onboarding, root.state.value)

        vm.onServerUrlChange("kb.example.org")
        vm.startPairing()
        assertEquals(OnboardingStep.Pairing, vm.state.value.step)
        assertEquals("https://kb.example.org", vm.state.value.serverUrl)
        assertEquals(PairingState.Waiting, vm.state.value.pairing?.state)
        assertTrue(vm.state.value.pairing?.code?.isNotBlank() == true)

        env.engine.advancePairing()
        assertEquals(PairingState.Confirming, vm.state.value.pairing?.state)
        assertEquals(6, vm.state.value.pairing?.sas?.length)

        vm.confirmPairing()
        assertEquals(PairingState.Done, vm.state.value.pairing?.state)
        assertEquals(RootUiState.Main, root.state.value)
    }

    @Test fun cancellingReturnsToTheServerStep() = runTest {
        val env = TestEnv(seed = false)
        val vm = OnboardingViewModel(env.spaces, env.sync)
        vm.onServerUrlChange("https://kb.example.org")
        vm.startPairing()
        vm.cancelPairing()
        assertEquals(OnboardingStep.Server, vm.state.value.step)
        assertNull(vm.state.value.pairing)
    }

    @Test fun startFreshCreatesTheWorkspaceWithItsFirstSpace() = runTest {
        val env = TestEnv(seed = false)
        val vm = OnboardingViewModel(env.spaces, env.sync)
        vm.onWorkspaceNameChange("  My notes ")
        vm.onSpaceNameChange("")
        vm.startFresh()
        assertEquals(listOf("Personal"), env.engine.spaces().map { it.name })
        assertNull(vm.state.value.error)
    }

    @Test fun startFreshNeedsAName() = runTest {
        val env = TestEnv(seed = false)
        val vm = OnboardingViewModel(env.spaces, env.sync)
        vm.startFresh()
        assertTrue(env.engine.spaces().isEmpty())
    }

    @Test fun rootFollowsTheEngineState() = runTest {
        val env = TestEnv()
        val root = RootViewModel(env.security, env.spaces)
        root.state.onEach { }.launchIn(backgroundScope + UnconfinedTestDispatcher(testScheduler))
        assertEquals(RootUiState.Main, root.state.value)
        env.engine.dispatchRaw("""{"type":"AppLockEnable","data":{"passphrase":"correct horse"}}""")
        env.engine.lock()
        assertEquals(EngineState.Locked, env.engine.state.value)
        assertEquals(RootUiState.Locked, root.state.value)
        env.engine.unlock("correct horse")
        assertEquals(RootUiState.Main, root.state.value)
    }
}
