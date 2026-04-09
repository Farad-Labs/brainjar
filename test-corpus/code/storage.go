// Package storage provides the Atlas storage layer, writing transformed events
// to ClickHouse for analytics and to S3 for long-term archival.
package storage

import (
	"context"
	"fmt"
	"log/slog"
	"sync"
	"time"
)

// DefaultFlushInterval is how often the buffer is flushed to the backend
// when no explicit flush is triggered.
const DefaultFlushInterval = 5 * time.Second

// MaxBatchSize is the maximum number of rows sent in a single write call.
const MaxBatchSize = 1_000

// RowID is a type alias for the unique identifier of a stored row.
type RowID = string

// TransformResult mirrors the structure produced by the transform engine.
// WHY: we duplicate the type here rather than importing it to keep the storage
// package free of transform-layer dependencies (dependency inversion).
type TransformResult struct {
	EventID      string
	Source       string
	Transformed  map[string]any
	RulesApplied []string
	LatencyMs    int64
}

// StorageBackend is the interface all write destinations must implement.
type StorageBackend interface {
	// Name returns a label used in metrics and logs.
	Name() string
	// Write persists a batch of results. Implementations must be goroutine-safe.
	Write(ctx context.Context, rows []TransformResult) error
	// Flush forces any buffered data to be committed.
	Flush(ctx context.Context) error
	// Close releases underlying connections.
	Close() error
}

// BufferedWriter wraps a StorageBackend with an in-memory buffer and
// periodic flush, reducing write amplification on the backend.
type BufferedWriter struct {
	backend       StorageBackend
	mu            sync.Mutex
	buf           []TransformResult
	flushInterval time.Duration
	logger        *slog.Logger
}

// NewBufferedWriter creates a BufferedWriter with the given backend.
func NewBufferedWriter(backend StorageBackend, flushInterval time.Duration) *BufferedWriter {
	return &BufferedWriter{
		backend:       backend,
		buf:           make([]TransformResult, 0, MaxBatchSize),
		flushInterval: flushInterval,
		logger:        slog.Default().With("component", "buffered_writer", "backend", backend.Name()),
	}
}

// Append adds a single result to the write buffer.
// If the buffer exceeds MaxBatchSize it is flushed immediately.
func (w *BufferedWriter) Append(ctx context.Context, result TransformResult) error {
	w.mu.Lock()
	w.buf = append(w.buf, result)
	full := len(w.buf) >= MaxBatchSize
	w.mu.Unlock()

	if full {
		return w.flush(ctx)
	}
	return nil
}

// flush drains the buffer to the backend. Caller must not hold w.mu.
func (w *BufferedWriter) flush(ctx context.Context) error {
	w.mu.Lock()
	if len(w.buf) == 0 {
		w.mu.Unlock()
		return nil
	}
	batch := w.buf
	w.buf = make([]TransformResult, 0, MaxBatchSize)
	w.mu.Unlock()

	start := time.Now()
	err := w.backend.Write(ctx, batch)
	if err != nil {
		// NOTE: on write failure we re-queue the batch to avoid data loss.
		// The calling goroutine is responsible for circuit-breaking if this
		// keeps failing (see supervisor.go).
		w.mu.Lock()
		w.buf = append(batch, w.buf...)
		w.mu.Unlock()
		return fmt.Errorf("backend write failed: %w", err)
	}

	w.logger.Info("flushed batch", "rows", len(batch), "latency_ms", time.Since(start).Milliseconds())
	return nil
}

// RunFlusher starts a background goroutine that flushes on the configured interval.
// The goroutine exits when ctx is cancelled.
func (w *BufferedWriter) RunFlusher(ctx context.Context) {
	go func() {
		ticker := time.NewTicker(w.flushInterval)
		defer ticker.Stop()
		for {
			select {
			case <-ticker.C:
				if err := w.flush(ctx); err != nil {
					w.logger.Error("periodic flush failed", "error", err)
				}
			case <-ctx.Done():
				// HACK: best-effort final flush; if the context is already
				// cancelled we use a short background context instead.
				_ = w.flush(context.Background())
				return
			}
		}
	}()
}
