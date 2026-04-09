/**
 * DataModels.kt — Atlas Kotlin data models and pipeline interfaces.
 *
 * Used by the Atlas Android client and the JVM-based ingestion microservice
 * to share a typed representation of pipeline events.
 */

package io.atlas.pipeline.models

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.JsonObject
import java.time.Instant
import java.util.UUID

// Type alias for event identifiers — keeps signatures readable.
typealias EventId = String

/** Maximum number of transformation rules that can be applied to a single event. */
const val MAX_RULES_PER_EVENT = 64

// ─────────────────────────────────────────────────────────────────────────────
// Data classes
// ─────────────────────────────────────────────────────────────────────────────

/**
 * Immutable snapshot of an event as it arrives from the ingestion layer.
 *
 * WHY: data class gives us equals/hashCode/copy for free, making it safe to
 * use IngestEvent as a map key and to produce modified copies in transform rules
 * without mutating the original.
 */
@Serializable
data class IngestEvent(
    val id: EventId = UUID.randomUUID().toString(),
    val source: String,
    val payload: JsonObject,
    val receivedAt: String = Instant.now().toString(),
    val metadata: Map<String, String> = emptyMap(),
)

/**
 * Result produced by the transformation engine for one event.
 */
@Serializable
data class TransformResult(
    val eventId: EventId,
    val source: String,
    val transformed: JsonObject,
    val rulesApplied: List<String> = emptyList(),
    val latencyMs: Long = 0L,
    val error: String? = null,
) {
    val success: Boolean get() = error == null
}

// ─────────────────────────────────────────────────────────────────────────────
// Interfaces
// ─────────────────────────────────────────────────────────────────────────────

/** Contract for Atlas pipeline stages operating on IngestEvents. */
interface EventProcessor {
    /** Human-readable processor name for metrics labelling. */
    val processorName: String

    /** Process a single event, suspending if I/O is required. */
    suspend fun process(event: IngestEvent): TransformResult

    /** Process a batch of events; implementations may parallelise internally. */
    suspend fun processBatch(events: List<IngestEvent>): List<TransformResult>
}

// ─────────────────────────────────────────────────────────────────────────────
// Companion object (factory / constants)
// ─────────────────────────────────────────────────────────────────────────────

/**
 * Default no-op processor used in tests and canary deployments.
 *
 * NOTE: the companion object exposes a singleton so callers don't need to
 * construct a new instance just to test the interface boundary.
 */
class PassThroughProcessor private constructor() : EventProcessor {
    override val processorName = "pass-through"

    override suspend fun process(event: IngestEvent): TransformResult =
        withContext(Dispatchers.Default) {
            TransformResult(
                eventId = event.id,
                source = event.source,
                transformed = event.payload,
                rulesApplied = listOf("pass-through"),
            )
        }

    override suspend fun processBatch(events: List<IngestEvent>): List<TransformResult> =
        events.map { process(it) }

    companion object {
        // HACK: lazy singleton avoids allocating the processor on class load in
        // environments where it will never be used (e.g., production workers that
        // always inject a real processor via DI).
        val INSTANCE: PassThroughProcessor by lazy { PassThroughProcessor() }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Extension functions
// ─────────────────────────────────────────────────────────────────────────────

/** Returns a copy of the event with additional metadata entries merged in. */
fun IngestEvent.withMetadata(extra: Map<String, String>): IngestEvent =
    copy(metadata = metadata + extra)

/** True if the result represents a validation or transform failure. */
val TransformResult.isError: Boolean get() = !success
