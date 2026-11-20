package io.atomik.kb.engine.model

import kotlinx.serialization.Serializable
import kotlinx.serialization.json.JsonElement

/** `kb_types::ErrorCode`. Unknown codes decode as [Internal]. */
@Serializable
enum class ErrorCode { NotFound, InvalidInput, AlreadyExists, Storage, Crypto, KeystoreUnavailable, Locked, Internal, Forbidden }

/** `kb_types::ApiError`. */
@Serializable
data class ApiError(
    val code: ErrorCode = ErrorCode.Internal,
    val message: String = "",
    val details: JsonElement? = null,
)
