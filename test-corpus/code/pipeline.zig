//! pipeline.zig — Atlas Zig ingestion pipeline.
//!
//! Provides a comptime-configurable pipeline that reads events from a ring
//! buffer, applies a sequence of transform functions, and writes results to
//! a storage backend.  Designed for embedded edge nodes where allocator
//! choice and binary size matter.

const std = @import("std");
const Allocator = std.mem.Allocator;
const assert = std.debug.assert;

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

/// Default ring-buffer capacity (must be a power of two).
pub const DEFAULT_BUFFER_CAPACITY: usize = 4096;

/// Maximum byte length of an event source identifier.
pub const MAX_SOURCE_LEN: usize = 128;

// ─────────────────────────────────────────────────────────────────────────────
// Error set
// ─────────────────────────────────────────────────────────────────────────────

pub const PipelineError = error{
    BufferFull,
    BufferEmpty,
    InvalidPayload,
    TransformFailed,
    StorageWriteFailed,
};

// ─────────────────────────────────────────────────────────────────────────────
// Data types
// ─────────────────────────────────────────────────────────────────────────────

/// A raw event as received from the network.
pub const IngestEvent = struct {
    id: u64,
    source: [MAX_SOURCE_LEN]u8,
    source_len: usize,
    payload: []const u8,
    received_at_ns: i128,

    /// Return a slice view of the source field without trailing zeros.
    pub fn sourceName(self: *const IngestEvent) []const u8 {
        return self.source[0..self.source_len];
    }
};

/// Result produced after a successful transform step.
pub const TransformResult = struct {
    event_id: u64,
    transformed: []const u8,
    rules_applied: u32,
    latency_ns: i64,
    success: bool,
};

// ─────────────────────────────────────────────────────────────────────────────
// Ring buffer
// ─────────────────────────────────────────────────────────────────────────────

/// Single-producer / single-consumer lock-free ring buffer.
///
/// WHY: we avoid std.atomic.Mutex here because on single-core MCUs the
/// overhead of a mutex is measurable; SPSC atomics suffice for our topology.
pub fn RingBuffer(comptime T: type, comptime capacity: usize) type {
    comptime assert(capacity > 0 and (capacity & (capacity - 1)) == 0); // power of two

    return struct {
        const Self = @This();
        const mask = capacity - 1;

        slots: [capacity]T = undefined,
        head: usize = 0,
        tail: usize = 0,

        pub fn push(self: *Self, item: T) PipelineError!void {
            const next = (self.tail + 1) & mask;
            if (next == self.head) return PipelineError.BufferFull;
            self.slots[self.tail] = item;
            self.tail = next;
        }

        pub fn pop(self: *Self) PipelineError!T {
            if (self.head == self.tail) return PipelineError.BufferEmpty;
            const item = self.slots[self.head];
            self.head = (self.head + 1) & mask;
            return item;
        }

        pub fn len(self: *const Self) usize {
            return (self.tail -% self.head) & mask;
        }
    };
}

// ─────────────────────────────────────────────────────────────────────────────
// Pipeline
// ─────────────────────────────────────────────────────────────────────────────

/// Comptime-parameterised pipeline.
///
/// *TransformFn* is a function pointer type: `fn(*const IngestEvent, Allocator) PipelineError!TransformResult`
pub fn Pipeline(comptime TransformFn: type) type {
    return struct {
        const Self = @This();

        buf: RingBuffer(IngestEvent, DEFAULT_BUFFER_CAPACITY) = .{},
        transform: TransformFn,
        allocator: Allocator,
        processed: u64 = 0,
        errors: u64 = 0,

        pub fn init(allocator: Allocator, transform: TransformFn) Self {
            return .{ .allocator = allocator, .transform = transform };
        }

        /// Enqueue one event for processing.
        pub fn ingest(self: *Self, event: IngestEvent) PipelineError!void {
            try self.buf.push(event);
        }

        /// Process one event from the queue.
        ///
        /// NOTE: returns `PipelineError.BufferEmpty` when the queue is drained;
        /// callers should treat that as a normal idle condition, not a fatal error.
        pub fn tick(self: *Self) PipelineError!TransformResult {
            const event = try self.buf.pop();
            const result = self.transform(&event, self.allocator) catch |err| {
                // HACK: we increment the error counter and re-raise; a proper DLQ
                // integration is planned for the next milestone (see issue #7).
                self.errors += 1;
                return err;
            };
            self.processed += 1;
            return result;
        }
    };
}
