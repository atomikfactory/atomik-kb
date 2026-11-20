package io.atomik.kb.data

import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import javax.inject.Inject
import javax.inject.Singleton

/** Holds shared content between the intent and the share screen (and across the lock screen). */
@Singleton
class ShareInbox @Inject constructor() {
    private val _content = MutableStateFlow<SharedContent?>(null)
    val content: StateFlow<SharedContent?> = _content.asStateFlow()
    fun set(value: SharedContent?) { _content.value = value }
    fun clear() { _content.value = null }
}
