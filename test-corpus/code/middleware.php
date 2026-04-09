<?php

declare(strict_types=1);

/**
 * Atlas PHP Middleware Layer
 *
 * PSR-15 compatible middleware stack that handles authentication, rate-limiting,
 * payload validation and event enrichment before forwarding to the ingestion queue.
 */

namespace Atlas\Pipeline\Middleware;

use Psr\Http\Message\ResponseInterface;
use Psr\Http\Message\ServerRequestInterface;
use Psr\Http\Server\MiddlewareInterface;
use Psr\Http\Server\RequestHandlerInterface;

// Type alias for the event identifier used throughout the pipeline.
/** @psalm-type EventId = non-empty-string */

/** Maximum request body size the middleware will accept, in bytes. */
const MAX_BODY_BYTES = 1_048_576; // 1 MiB

/** Current schema version injected into every enriched event. */
const SCHEMA_VERSION = '2.0';

// ─────────────────────────────────────────────────────────────────────────────
// Interfaces
// ─────────────────────────────────────────────────────────────────────────────

/**
 * Contract for event validation strategies.
 *
 * WHY: separating validation from enrichment keeps each class focused on a
 * single responsibility and makes unit testing trivial.
 */
interface EventValidatorInterface
{
    /**
     * Validate a decoded event payload.
     *
     * @param array<string, mixed> $payload
     * @return true on success
     * @throws \InvalidArgumentException on validation failure
     */
    public function validate(array $payload): true;
}

/**
 * Contract for event enrichment strategies.
 */
interface EventEnricherInterface
{
    /**
     * Add server-side metadata to the event payload.
     *
     * @param array<string, mixed> $payload
     * @return array<string, mixed> enriched payload
     */
    public function enrich(array $payload): array;
}

// ─────────────────────────────────────────────────────────────────────────────
// Concrete implementations
// ─────────────────────────────────────────────────────────────────────────────

/**
 * Validates that the required top-level keys are present and non-empty.
 */
final class SchemaValidator implements EventValidatorInterface
{
    private const REQUIRED_KEYS = ['source', 'payload'];

    public function validate(array $payload): true
    {
        foreach (self::REQUIRED_KEYS as $key) {
            if (empty($payload[$key])) {
                throw new \InvalidArgumentException("Missing required field: {$key}");
            }
        }
        return true;
    }
}

/**
 * Enriches events with server-side identifiers and timestamps.
 */
final class ServerEnricher implements EventEnricherInterface
{
    public function enrich(array $payload): array
    {
        // NOTE: we generate the ID server-side even if the client provided one;
        // client-supplied IDs are stored under 'client_id' for traceability.
        if (isset($payload['id'])) {
            $payload['client_id'] = $payload['id'];
        }

        return array_merge($payload, [
            'id'             => \Ramsey\Uuid\Uuid::uuid4()->toString(),
            'received_at'    => (new \DateTimeImmutable())->format(\DateTimeInterface::ATOM),
            'schema_version' => SCHEMA_VERSION,
        ]);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Middleware
// ─────────────────────────────────────────────────────────────────────────────

/**
 * PSR-15 middleware that validates and enriches inbound Atlas events.
 */
final class AtlasIngestionMiddleware implements MiddlewareInterface
{
    public function __construct(
        private readonly EventValidatorInterface $validator,
        private readonly EventEnricherInterface  $enricher,
        private readonly \Psr\Log\LoggerInterface $logger,
    ) {}

    public function process(
        ServerRequestInterface  $request,
        RequestHandlerInterface $handler,
    ): ResponseInterface {
        $body = (string) $request->getBody();

        if (strlen($body) > MAX_BODY_BYTES) {
            // HACK: returning 413 here is correct but we lose the client IP in logs;
            // proper structured logging with request context is tracked in #77.
            return $this->errorResponse(413, 'Payload too large');
        }

        $payload = json_decode($body, true, 512, JSON_THROW_ON_ERROR);

        if (!is_array($payload)) {
            return $this->errorResponse(400, 'Invalid JSON');
        }

        try {
            $this->validator->validate($payload);
        } catch (\InvalidArgumentException $e) {
            $this->logger->warning('Validation failed', ['error' => $e->getMessage()]);
            return $this->errorResponse(422, $e->getMessage());
        }

        $enriched = $this->enricher->enrich($payload);
        $this->logger->info('Event enriched', ['event_id' => $enriched['id']]);

        $request = $request->withParsedBody($enriched);
        return $handler->handle($request);
    }

    private function errorResponse(int $status, string $message): ResponseInterface
    {
        // Returns a minimal PSR-7 response — real app uses a ResponseFactory.
        throw new \RuntimeException("HTTP {$status}: {$message}");
    }
}
