/**
 * ingestion.c — Atlas C ingestion shim.
 *
 * Low-level C layer that bridges network I/O (libuv) with the Atlas
 * ingestion queue. Used in the embedded agent that runs on edge nodes
 * with limited runtimes.
 *
 * Function prototypes:
 *   atlas_event_t *atlas_event_create(const char *source, const char *payload, size_t len);
 *   int            atlas_event_enqueue(atlas_queue_t *q, atlas_event_t *evt);
 *   void           atlas_event_free(atlas_event_t *evt);
 *   int            atlas_queue_flush(atlas_queue_t *q);
 */

#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

/* Maximum bytes allowed in a single event payload. */
#define ATLAS_MAX_PAYLOAD_BYTES (1024 * 1024)

/* Default queue capacity (number of events). */
#define ATLAS_QUEUE_CAPACITY 4096

/* Error codes returned by ingestion functions. */
#define ATLAS_OK            0
#define ATLAS_ERR_NOMEM    -1
#define ATLAS_ERR_OVERFLOW -2
#define ATLAS_ERR_INVALID  -3

/** Opaque identifier type for events. */
typedef uint64_t atlas_event_id_t;

/**
 * Represents a single ingest event in C.
 *
 * WHY: we store the payload as a flexible array member to avoid a second
 * allocation — on edge nodes malloc latency is measurable at high throughput.
 */
typedef struct atlas_event {
    atlas_event_id_t id;
    char             source[128];
    time_t           received_at;
    size_t           payload_len;
    uint8_t          payload[];   /* flexible array member */
} atlas_event_t;

/**
 * Simple ring-buffer queue for atlas events.
 *
 * NOTE: this is intentionally single-producer / single-consumer to avoid
 * locking overhead on embedded targets. Use the thread-safe wrapper in
 * atlas_queue_mt.c for multi-threaded deployments.
 */
typedef struct atlas_queue {
    atlas_event_t **slots;
    size_t          capacity;
    size_t          head;
    size_t          tail;
} atlas_queue_t;

/* -------------------------------------------------------------------------
 * Internal helpers
 * ------------------------------------------------------------------------- */

static atlas_event_id_t next_event_id(void) {
    /* HACK: monotonic counter — replace with UUID generator once libuuid is
     * available on all target platforms. Tracked in issue #88. */
    static atlas_event_id_t counter = 0;
    return ++counter;
}

/* -------------------------------------------------------------------------
 * Public API
 * ------------------------------------------------------------------------- */

/**
 * Allocate and initialise a new event.
 *
 * @param source   Null-terminated source identifier string.
 * @param payload  Raw payload bytes (may be NULL if len == 0).
 * @param len      Number of bytes in payload.
 * @return Pointer to allocated event, or NULL on error.
 */
atlas_event_t *atlas_event_create(const char *source, const uint8_t *payload, size_t len) {
    if (!source || len > ATLAS_MAX_PAYLOAD_BYTES) {
        return NULL;
    }

    atlas_event_t *evt = malloc(sizeof(atlas_event_t) + len);
    if (!evt) {
        return NULL;
    }

    evt->id          = next_event_id();
    evt->received_at = time(NULL);
    evt->payload_len = len;
    strncpy(evt->source, source, sizeof(evt->source) - 1);
    evt->source[sizeof(evt->source) - 1] = '\0';

    if (len > 0 && payload) {
        memcpy(evt->payload, payload, len);
    }

    return evt;
}

/**
 * Enqueue an event into the ring buffer.
 *
 * @return ATLAS_OK, ATLAS_ERR_OVERFLOW if full, ATLAS_ERR_INVALID if args bad.
 */
int atlas_event_enqueue(atlas_queue_t *q, atlas_event_t *evt) {
    if (!q || !evt) return ATLAS_ERR_INVALID;

    size_t next_tail = (q->tail + 1) % q->capacity;
    if (next_tail == q->head) {
        return ATLAS_ERR_OVERFLOW;
    }

    q->slots[q->tail] = evt;
    q->tail = next_tail;
    return ATLAS_OK;
}

/** Free an event allocated by atlas_event_create. */
void atlas_event_free(atlas_event_t *evt) {
    free(evt);
}
