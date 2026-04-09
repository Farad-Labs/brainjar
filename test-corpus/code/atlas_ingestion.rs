//! Atlas Ingestion Layer
//!
//! Handles receiving events from HTTP and Kafka sources, validating them,
//! and forwarding to the transformation pipeline.

use std::collections::HashMap;
use std::time::{Duration, SystemTime};
use tokio::sync::mpsc;
use serde::{Deserialize, Serialize};

/// Maximum number of events buffered in the ingestion channel before back-pressure kicks in.
const INGESTION_BUFFER_SIZE: usize = 4096;

/// Default timeout for upstream acknowledgement, in milliseconds.
const ACK_TIMEOUT_MS: u64 = 5000;

/// A type alias for the event identifier, used throughout the pipeline.
pub type EventId = uuid::Uuid;

/// Represents a raw event arriving from an external producer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestEvent {
    pub id: EventId,
    pub source: String,
    pub payload: serde_json::Value,
    pub received_at: SystemTime,
    pub metadata: HashMap<String, String>,
}

/// Result type returned after processing an ingested event.
#[derive(Debug)]
pub struct IngestResult {
    pub event_id: EventId,
    pub success: bool,
    pub latency_ms: u64,
}

/// Trait that all ingestion backends must implement.
///
/// Implementations exist for HTTP (webhook) and Kafka consumers.
pub trait IngestionBackend: Send + Sync {
    /// Poll for the next available event. Returns `None` on timeout.
    fn poll(&mut self) -> Option<IngestEvent>;

    /// Acknowledge successful processing of an event by ID.
    fn ack(&mut self, event_id: EventId) -> Result<(), IngestionError>;

    /// Return a human-readable name for metrics labelling.
    fn backend_name(&self) -> &str;
}

/// Errors that can occur during ingestion.
#[derive(Debug, thiserror::Error)]
pub enum IngestionError {
    #[error("connection lost: {0}")]
    ConnectionLost(String),
    #[error("deserialisation failed: {0}")]
    DeserFailed(String),
    #[error("ack timeout after {0}ms")]
    AckTimeout(u64),
}

/// The main ingestion coordinator. Owns one or more backends and fans
/// events into a shared channel consumed by the transform engine.
pub struct IngestionCoordinator {
    backends: Vec<Box<dyn IngestionBackend>>,
    sender: mpsc::Sender<IngestEvent>,
    buffer_size: usize,
}

impl IngestionCoordinator {
    /// Create a new coordinator with the given backends.
    ///
    /// # WHY: we accept a pre-built sender so the coordinator is testable
    /// without spinning up a real transform engine.
    pub fn new(
        backends: Vec<Box<dyn IngestionBackend>>,
        sender: mpsc::Sender<IngestEvent>,
    ) -> Self {
        Self {
            backends,
            sender,
            buffer_size: INGESTION_BUFFER_SIZE,
        }
    }

    /// Validate an event before forwarding it downstream.
    fn validate(&self, event: &IngestEvent) -> bool {
        // NOTE: We intentionally allow empty payloads — downstream consumers
        // handle schema enforcement; ingestion is deliberately permissive.
        !event.source.is_empty()
    }

    /// Process a single event: validate, tag, and forward.
    async fn process_event(&self, event: IngestEvent) -> IngestResult {
        let start = SystemTime::now();
        let event_id = event.id;

        if !self.validate(&event) {
            return IngestResult {
                event_id,
                success: false,
                latency_ms: 0,
            };
        }

        let success = self.sender.send(event).await.is_ok();
        let latency_ms = start
            .elapsed()
            .unwrap_or(Duration::from_millis(0))
            .as_millis() as u64;

        IngestResult {
            event_id,
            success,
            latency_ms,
        }
    }

    /// Run the ingestion loop indefinitely, polling all backends in round-robin.
    pub async fn run(&mut self) {
        loop {
            for backend in self.backends.iter_mut() {
                if let Some(event) = backend.poll() {
                    let result = self.process_event(event).await;
                    if !result.success {
                        // HACK: log and continue — proper DLQ integration is tracked in #204
                        eprintln!(
                            "[{}] failed to forward event {}",
                            backend.backend_name(),
                            result.event_id
                        );
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}
