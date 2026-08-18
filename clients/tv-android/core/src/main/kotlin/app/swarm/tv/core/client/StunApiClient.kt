/**
 * Device-side REST client for the SWARM STUN server. Mirrors
 * `swarm-stun-client::client::StunClient` (Rust) method-for-method: register
 * a device with a join code, join an additional swarm, fetch a swarm's
 * device roster. WSS signaling is a separate concern landing with the
 * hole-punch work (see the Rust crate's module docs) — this covers the REST
 * half only, same as the Rust side today.
 */
package app.swarm.tv.core.client

import app.swarm.tv.core.rest.ApiError
import app.swarm.tv.core.rest.JoinSwarmRequest
import app.swarm.tv.core.rest.RegisterDeviceRequest
import app.swarm.tv.core.rest.RegisterDeviceResponse
import app.swarm.tv.core.rest.DeviceRegistration
import app.swarm.tv.core.rest.SwarmDevicesResponse
import app.swarm.tv.core.rest.SwarmJson
import app.swarm.tv.core.rest.SwarmSummary
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.serialization.SerializationException
import kotlinx.serialization.decodeFromString
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.JsonElement
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import java.io.IOException

sealed class StunClientError(message: String) : Exception(message) {
    class Network(message: String) : StunClientError(message)
    class Api(val status: Int, val code: String, val apiMessage: String) : StunClientError(apiMessage)
    class Decode(message: String) : StunClientError(message)

    /** The caller's cue to drop the stored token and prompt for a fresh join code. */
    val isUnauthorized: Boolean get() = this is Api && status == 401
}

class StunApiClient(
    baseUrl: String,
    private val http: OkHttpClient = OkHttpClient(),
) {
    private val base = baseUrl.trimEnd('/')
    private val jsonMediaType = "application/json".toMediaType()

    suspend fun registerDevice(code: String, device: DeviceRegistration): RegisterDeviceResponse =
        postJson("/api/v1/devices/register", RegisterDeviceRequest(code, device), bearer = null)

    suspend fun joinSwarm(accessToken: String, code: String): SwarmSummary =
        postJson("/api/v1/swarms/join", JoinSwarmRequest(code), bearer = accessToken)

    suspend fun swarmDevices(accessToken: String, swarmId: String): SwarmDevicesResponse =
        getJson("/api/v1/swarms/$swarmId/devices", bearer = accessToken)

    /** Leave one swarm this device belongs to, keeping the rest of its memberships and its access token intact. */
    suspend fun leaveSwarm(accessToken: String, swarmId: String, deviceId: String) {
        deleteJson<JsonElement>("/api/v1/swarms/$swarmId/devices/$deviceId", bearer = accessToken)
    }

    private suspend inline fun <reified Req, reified Resp> postJson(
        path: String,
        body: Req,
        bearer: String?,
    ): Resp {
        val requestBody = SwarmJson.encodeToString(body).toRequestBody(jsonMediaType)
        val builder = Request.Builder().url("$base$path").post(requestBody)
        bearer?.let { builder.header("Authorization", "Bearer $it") }
        return execute(builder.build())
    }

    private suspend inline fun <reified Resp> getJson(path: String, bearer: String?): Resp {
        val builder = Request.Builder().url("$base$path").get()
        bearer?.let { builder.header("Authorization", "Bearer $it") }
        return execute(builder.build())
    }

    private suspend inline fun <reified Resp> deleteJson(path: String, bearer: String?): Resp {
        val builder = Request.Builder().url("$base$path").delete()
        bearer?.let { builder.header("Authorization", "Bearer $it") }
        return execute(builder.build())
    }

    private suspend inline fun <reified Resp> execute(request: Request): Resp = withContext(Dispatchers.IO) {
        val response = try {
            http.newCall(request).execute()
        } catch (e: IOException) {
            throw StunClientError.Network(e.message ?: "network error")
        }
        response.use {
            val bodyText = it.body?.string().orEmpty()
            if (it.isSuccessful) {
                try {
                    SwarmJson.decodeFromString<Resp>(bodyText)
                } catch (e: SerializationException) {
                    throw StunClientError.Decode(e.message ?: "malformed response body")
                }
            } else {
                val apiError = runCatching { SwarmJson.decodeFromString<ApiError>(bodyText) }.getOrNull()
                throw StunClientError.Api(
                    status = it.code,
                    code = apiError?.code ?: "unknown",
                    apiMessage = apiError?.message ?: "request failed with status ${it.code}",
                )
            }
        }
    }
}
