package io.atlas.pipeline;

import java.time.Instant;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.logging.Logger;

/**
 * Atlas DataProcessor: applies validation and enrichment rules to ingested events
 * before they enter the transformation engine.
 *
 * <p>Implementations choose between synchronous (blocking) and asynchronous
 * variants by implementing either {@link DataProcessor} or wrapping it in
 * {@link AsyncDataProcessor}.
 */
public class DataProcessor implements EventProcessor {

    private static final Logger LOG = Logger.getLogger(DataProcessor.class.getName());

    /** Maximum payload size (bytes) accepted by the processor. */
    public static final int MAX_PAYLOAD_BYTES = 1_048_576; // 1 MiB

    /** Type alias expressed as a constant — real alias via {@code typedef} not available in Java. */
    public static final String PROCESSOR_TYPE = "atlas-data-processor-v2";

    // WHY: version is embedded in the processor output so downstream consumers
    // can handle schema migrations without breaking existing pipelines.
    private final String version;
    private final List<ValidationRule> rules;
    private int processedCount = 0;

    public DataProcessor(String version, List<ValidationRule> rules) {
        this.version = Objects.requireNonNull(version);
        this.rules = new ArrayList<>(rules);
    }

    /**
     * Validate a raw event payload against all registered rules.
     *
     * @param event the inbound event to validate
     * @return {@code true} if all enabled rules pass
     */
    protected boolean validate(IngestEvent event) {
        for (ValidationRule rule : rules) {
            if (rule.isEnabled() && !rule.evaluate(event)) {
                LOG.warning("validation failed: rule=" + rule.getName() + " eventId=" + event.getId());
                return false;
            }
        }
        return true;
    }

    /**
     * Enrich the event with server-side metadata before forwarding.
     *
     * NOTE: enrichment runs after validation so we never store metadata
     * for events we will ultimately reject.
     */
    private IngestEvent enrich(IngestEvent event) {
        Map<String, String> meta = event.getMetadata();
        meta.put("processor_version", version);
        meta.put("processed_at", Instant.now().toString());
        return event.withMetadata(meta);
    }

    /**
     * Process a single event: validate then enrich.
     *
     * @param event raw inbound event
     * @return {@link ProcessResult} describing the outcome
     */
    @Override
    public ProcessResult process(IngestEvent event) {
        if (!validate(event)) {
            return ProcessResult.rejected(event.getId(), "validation_failed");
        }

        IngestEvent enriched = enrich(event);
        processedCount++;
        return ProcessResult.accepted(enriched);
    }

    /**
     * Process a list of events and return results in the same order.
     *
     * @param events list of raw events
     * @return list of results, one per input event
     */
    @Override
    public List<ProcessResult> processBatch(List<IngestEvent> events) {
        List<ProcessResult> results = new ArrayList<>(events.size());
        for (IngestEvent event : events) {
            results.add(process(event));
        }
        return results;
    }

    /** Returns the total number of successfully processed events since creation. */
    public int getProcessedCount() {
        return processedCount;
    }

    // -------------------------------------------------------------------------
    // Inner interfaces and value types
    // -------------------------------------------------------------------------

    /** Contract for synchronous event processing. */
    public interface EventProcessor {
        ProcessResult process(IngestEvent event);
        List<ProcessResult> processBatch(List<IngestEvent> events);
    }

    /** A single validation rule that can be enabled or disabled at runtime. */
    public interface ValidationRule {
        String getName();
        boolean isEnabled();
        boolean evaluate(IngestEvent event);
    }

    /** Immutable result of processing one event. */
    public record ProcessResult(String eventId, boolean accepted, String reason, IngestEvent enriched) {

        public static ProcessResult accepted(IngestEvent event) {
            return new ProcessResult(event.getId(), true, "ok", event);
        }

        public static ProcessResult rejected(String eventId, String reason) {
            return new ProcessResult(eventId, false, reason, null);
        }
    }
}
