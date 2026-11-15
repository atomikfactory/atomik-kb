package io.atomik.kb.engine.model

import kotlinx.serialization.json.Json

/** The JSON configuration that matches `kb_types` serde: absent options, unknown keys ignored. */
val CoreJson: Json = Json {
    ignoreUnknownKeys = true
    explicitNulls = false
    encodeDefaults = false
    coerceInputValues = true
    isLenient = false
}
