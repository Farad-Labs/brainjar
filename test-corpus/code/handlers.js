/**
 * handlers.js — Atlas HTTP ingestion handlers.
 *
 * Express-compatible route handlers that receive webhook events, validate
 * them, and forward them to the internal ingestion queue.
 *
 * CommonJS interop note: this module uses ES module syntax at the top level
 * but is compiled to CJS by the build step for Node 18 compatibility.
 */

import crypto from "node:crypto";
import { createLogger } from "./logger.js";
import { IngestQueue } from "./ingest_queue.js";

const logger = createLogger("handlers");

/** HMAC algorithm used to verify webhook signatures from upstream producers. */
const SIGNATURE_ALGO = "sha256";

/**
 * Shared ingestion queue instance.
 *
 * WHY: singleton pattern here keeps the queue reference stable across hot
 * reloads in development without leaking connections.
 */
const queue = new IngestQueue({ bufferSize: 4096 });

/**
 * Verify a webhook signature against the raw request body.
 *
 * @param {Buffer} body   - Raw request body bytes.
 * @param {string} sig    - Hex-encoded HMAC from the `X-Atlas-Signature` header.
 * @param {string} secret - Shared secret stored in the Atlas config.
 * @returns {boolean}
 */
const verifySignature = (body, sig, secret) => {
  const expected = crypto
    .createHmac(SIGNATURE_ALGO, secret)
    .update(body)
    .digest("hex");
  // NOTE: use timingSafeEqual to prevent timing attacks on signature comparison.
  return crypto.timingSafeEqual(Buffer.from(sig, "hex"), Buffer.from(expected, "hex"));
};

/**
 * Parse and enrich an incoming event object.
 *
 * @param {object} raw      - Parsed JSON body.
 * @param {string} sourceIp - Originating IP for audit logging.
 * @returns {object} Enriched event ready for the queue.
 */
const buildIngestEvent = (raw, sourceIp) => ({
  id: crypto.randomUUID(),
  source: raw.source ?? "unknown",
  payload: raw.payload ?? {},
  receivedAt: new Date().toISOString(),
  metadata: {
    sourceIp,
    contentType: "application/json",
  },
});

/**
 * POST /ingest
 *
 * Main webhook endpoint. Validates signature, builds the event, and enqueues it.
 */
export const handleIngest = async (req, res) => {
  const sig = req.headers["x-atlas-signature"];
  const secret = process.env.ATLAS_WEBHOOK_SECRET;

  if (!sig || !secret) {
    return res.status(400).json({ error: "missing signature header or secret" });
  }

  let bodyBuffer;
  try {
    bodyBuffer = Buffer.from(JSON.stringify(req.body));
  } catch {
    return res.status(400).json({ error: "invalid JSON body" });
  }

  if (!verifySignature(bodyBuffer, sig, secret)) {
    logger.warn("signature mismatch from %s", req.ip);
    return res.status(401).json({ error: "invalid signature" });
  }

  const event = buildIngestEvent(req.body, req.ip);

  try {
    await queue.enqueue(event);
    logger.info("enqueued event %s from %s", event.id, event.source);
    return res.status(202).json({ eventId: event.id });
  } catch (err) {
    // HACK: swallowing the queue-full error here and returning 503 is a
    // short-term fix; proper back-pressure via HTTP 429 + Retry-After is
    // tracked in issue #311.
    logger.error("queue full, dropping event: %s", err.message);
    return res.status(503).json({ error: "service unavailable" });
  }
};

/**
 * GET /health
 *
 * Liveness probe used by Kubernetes. Always 200 if the process is alive.
 */
export const handleHealth = (_req, res) => {
  res.status(200).json({ status: "ok", queueDepth: queue.depth() });
};

export default { handleIngest, handleHealth };
