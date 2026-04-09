// EventBus.cs — Atlas C# in-process event bus.
//
// Provides a lightweight publish-subscribe mechanism that decouples the
// ingestion layer from the transformation and storage layers within a
// single Atlas worker process.

using System;
using System.Collections.Concurrent;
using System.Collections.Generic;
using System.Linq;
using System.Text.Json;
using System.Threading;
using System.Threading.Tasks;
using Microsoft.Extensions.Logging;

namespace Atlas.Pipeline;

// Type alias for the subscriber delegate — kept short for readability in handler registrations.
using EventHandler = Func<IngestEvent, CancellationToken, Task>;

/// <summary>
/// Immutable record representing one event flowing through the Atlas pipeline.
/// </summary>
public sealed record IngestEvent(
    string Id,
    string Source,
    JsonDocument Payload,
    DateTimeOffset ReceivedAt,
    IReadOnlyDictionary<string, string> Metadata
);

/// <summary>
/// Result produced after a subscriber processes an event.
/// </summary>
public sealed record ProcessResult(
    string EventId,
    bool Success,
    string? Error = null,
    long LatencyMs = 0
);

/// <summary>
/// Contract for event bus implementations.
///
/// <para>WHY: abstracting behind an interface lets tests inject a synchronous
/// in-memory bus without spinning up real queue infrastructure.</para>
/// </summary>
public interface IEventBus
{
    /// <summary>Subscribe <paramref name="handler"/> to events matching <paramref name="source"/>.</summary>
    IDisposable Subscribe(string source, EventHandler handler);

    /// <summary>Publish an event to all matching subscribers.</summary>
    Task PublishAsync(IngestEvent @event, CancellationToken ct = default);

    /// <summary>Return current subscriber count for observability.</summary>
    int SubscriberCount { get; }
}

/// <summary>
/// Concurrent in-process event bus backed by a <see cref="ConcurrentDictionary"/>.
/// </summary>
public sealed class EventBus : IEventBus, IAsyncDisposable
{
    private readonly ConcurrentDictionary<string, List<EventHandler>> _subscribers = new();
    private readonly ILogger<EventBus> _logger;

    // NOTE: we use a semaphore rather than lock() to keep publish fully async
    // and avoid blocking the thread-pool during high-throughput bursts.
    private readonly SemaphoreSlim _gate = new(1, 1);

    public EventBus(ILogger<EventBus> logger)
    {
        _logger = logger;
    }

    /// <inheritdoc/>
    public int SubscriberCount =>
        _subscribers.Values.Sum(list => list.Count);

    /// <inheritdoc/>
    public IDisposable Subscribe(string source, EventHandler handler)
    {
        _subscribers.AddOrUpdate(
            source,
            _ => [handler],
            (_, existing) => { existing.Add(handler); return existing; }
        );

        _logger.LogDebug("Subscribed handler for source {Source}", source);
        return new Subscription(() => Unsubscribe(source, handler));
    }

    /// <inheritdoc/>
    public async Task PublishAsync(IngestEvent @event, CancellationToken ct = default)
    {
        var handlers = GetHandlers(@event.Source).ToList();

        if (handlers.Count == 0)
        {
            _logger.LogWarning("No handlers for source {Source}, event {Id} dropped", @event.Source, @event.Id);
            return;
        }

        // HACK: fire-and-forget per handler; a proper implementation would
        // collect results and surface failures to the DLQ (tracked in #412).
        var tasks = handlers.Select(h => InvokeHandler(h, @event, ct));
        await Task.WhenAll(tasks).ConfigureAwait(false);
    }

    private async Task InvokeHandler(EventHandler handler, IngestEvent @event, CancellationToken ct)
    {
        try
        {
            await handler(@event, ct).ConfigureAwait(false);
        }
        catch (Exception ex)
        {
            _logger.LogError(ex, "Handler threw for event {Id}", @event.Id);
        }
    }

    private IEnumerable<EventHandler> GetHandlers(string source) =>
        _subscribers
            .Where(kv => source.StartsWith(kv.Key, StringComparison.OrdinalIgnoreCase))
            .SelectMany(kv => kv.Value);

    private void Unsubscribe(string source, EventHandler handler)
    {
        if (_subscribers.TryGetValue(source, out var list))
            list.Remove(handler);
    }

    public async ValueTask DisposeAsync()
    {
        _gate.Dispose();
        await Task.CompletedTask;
    }

    private sealed class Subscription(Action onDispose) : IDisposable
    {
        public void Dispose() => onDispose();
    }
}
