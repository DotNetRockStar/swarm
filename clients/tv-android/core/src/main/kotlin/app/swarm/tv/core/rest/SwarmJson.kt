package app.swarm.tv.core.rest

import kotlinx.serialization.ExperimentalSerializationApi
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonNamingStrategy

/**
 * The one [Json] configuration every SWARM wire type uses: snake_case on
 * the wire (matching serde's `rename_all`), unknown response fields
 * ignored (the extensible-response half of the contract discipline in
 * `swarm-core::rest`'s module docs — requests are strict by construction
 * here, since we only ever encode the fields our data classes declare).
 *
 * `explicitNulls = false` matches the `#[serde(skip_serializing_if =
 * "Option::is_none")]` most optional Rust fields use — a null-valued
 * optional field with a Kotlin default is omitted rather than encoded as
 * `"field":null`. One Rust type (`SwarmDevice.last_seen_at`) does *not* use
 * that attribute and always emits an explicit `null`, but this side only
 * ever *decodes* that type, and a missing key vs. an explicit null decode
 * identically here, so the global setting is safe.
 */
@OptIn(ExperimentalSerializationApi::class)
val SwarmJson: Json = Json {
    namingStrategy = JsonNamingStrategy.SnakeCase
    ignoreUnknownKeys = true
    encodeDefaults = true
    explicitNulls = false
}
