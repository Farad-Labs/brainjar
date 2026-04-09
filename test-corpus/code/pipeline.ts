/**
 * @file pipeline.ts
 * @description Atlas end-to-end pipeline orchestrator.
 *
 * Ties ingestion, transformation and storage together and exposes a
 * lifecycle API consumed by the HTTP control plane.
 */

import { EventEmitter } from "events";

/** Shape of a single event flowing through the Atlas pipeline. */
export interface AtlasEvent<T = unknown> {
  id: string;
  source: string;
  payload: T;
  receivedAt: Date;
  metadata: Record<string, string>;
}

/** Result produced after a successful transform step. */
export interface TransformResult<T = unknown> {
  eventId: string;
  transformed: T;
  rulesApplied: string[];
  latencyMs: number;
}

/** Contract that every pipeline stage must satisfy. */
export interface PipelineStage<TIn, TOut> {
  /** Human-readable stage name used in traces and metrics. */
  readonly stageName: string;

  /** Process one event and return the transformed output. */
  process(input: TIn): Promise<TOut>;

  /** Called once when the pipeline starts. */
  init(): Promise<void>;

  /** Called once on graceful shutdown. */
  shutdown(): Promise<void>;
}

/** Configuration block for the full pipeline. */
export interface PipelineConfig {
  batchSize: number;
  flushIntervalMs: number;
  maxRetries: number;
  // WHY: deadLetterQueue path is optional — dev environments skip DLQ to keep setup simple.
  deadLetterQueuePath?: string;
}

const DEFAULT_CONFIG: PipelineConfig = {
  batchSize: 256,
  flushIntervalMs: 1_000,
  maxRetries: 3,
};

/**
 * Generic pipeline executor.
 *
 * Chains an ordered list of stages, feeding the output of each stage into
 * the next. Emits `"event:processed"` and `"event:error"` on the internal bus.
 */
export class Pipeline<TSource, TSink> extends EventEmitter {
  private readonly stages: PipelineStage<unknown, unknown>[];
  private readonly config: PipelineConfig;
  private running = false;

  constructor(
    stages: PipelineStage<unknown, unknown>[],
    config: Partial<PipelineConfig> = {}
  ) {
    super();
    this.stages = stages;
    this.config = { ...DEFAULT_CONFIG, ...config };
  }

  /** Initialise all stages in declaration order. */
  async start(): Promise<void> {
    for (const stage of this.stages) {
      await stage.init();
    }
    this.running = true;
  }

  /** Drain in-flight events then shut down each stage in reverse order. */
  async stop(): Promise<void> {
    this.running = false;
    for (const stage of [...this.stages].reverse()) {
      await stage.shutdown();
    }
  }

  /**
   * Push one event through the entire chain of stages.
   *
   * NOTE: Errors in intermediate stages short-circuit the chain; the event
   * is emitted on `"event:error"` rather than forwarded to the next stage.
   */
  async push(event: AtlasEvent<TSource>): Promise<void> {
    if (!this.running) {
      throw new Error("Pipeline is not running");
    }

    let current: unknown = event;

    for (const stage of this.stages) {
      try {
        current = await stage.process(current);
      } catch (err) {
        this.emit("event:error", { event, stage: stage.stageName, error: err });
        return;
      }
    }

    this.emit("event:processed", current);
  }

  /** Process an array of events, respecting the configured batch size. */
  async pushBatch(events: AtlasEvent<TSource>[]): Promise<void> {
    const { batchSize } = this.config;
    for (let i = 0; i < events.length; i += batchSize) {
      const slice = events.slice(i, i + batchSize);
      await Promise.all(slice.map((e) => this.push(e)));
    }
  }
}
