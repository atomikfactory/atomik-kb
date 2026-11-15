package io.atomik.kb.ui.nav

import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.receiveAsFlow
import javax.inject.Inject
import javax.inject.Singleton

/** Carries what an intent asked for to the navigation host, which may not exist yet (lock, onboarding). */
@Singleton
class IntentRouter @Inject constructor() {
    private val channel = Channel<NavRequest>(Channel.BUFFERED)
    val requests: Flow<NavRequest> = channel.receiveAsFlow()
    fun request(request: NavRequest) { channel.trySend(request) }
}
