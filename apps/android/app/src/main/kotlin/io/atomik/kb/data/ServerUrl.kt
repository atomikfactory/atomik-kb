package io.atomik.kb.data

import java.net.URI

/**
 * Turns what the user typed into a server URL the core accepts (`http(s)://host[:port]`), or null.
 * A bare host gets `https://`; paths, queries and trailing slashes are dropped.
 */
fun normalizeServerUrl(input: String): String? {
    val text = input.trim()
    if (text.isEmpty() || text.any { it.isWhitespace() }) return null
    val withScheme = if (text.contains("://")) text else "https://$text"
    val uri = try { URI(withScheme) } catch (_: Exception) { return null }
    val scheme = uri.scheme?.lowercase() ?: return null
    if (scheme != "http" && scheme != "https") return null
    val host = uri.host?.takeIf { it.isNotEmpty() } ?: return null
    val port = if (uri.port > 0) ":${uri.port}" else ""
    return "$scheme://${host.lowercase()}$port"
}
