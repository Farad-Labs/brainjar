/**
 * metrics.scala — Atlas pipeline metrics collection.
 *
 * Provides a lightweight, immutable metrics model and a collector that
 * accumulates counters/gauges from each pipeline stage. Used to feed
 * Prometheus via the /metrics endpoint and to power internal dashboards.
 */

package io.atlas.pipeline.metrics

import scala.collection.concurrent.TrieMap
import java.time.Instant
import java.util.concurrent.atomic.LongAdder

// ─────────────────────────────────────────────────────────────────────────────
// Type aliases
// ─────────────────────────────────────────────────────────────────────────────

/** A metric name following the Prometheus naming convention. */
type MetricName = String

/** A snapshot of label key-value pairs attached to a metric. */
type Labels = Map[String, String]

// ─────────────────────────────────────────────────────────────────────────────
// Sealed trait hierarchy
// ─────────────────────────────────────────────────────────────────────────────

/**
 * Base trait for all Atlas metric types.
 *
 * WHY: sealed so the compiler can exhaustively check pattern matches —
 * adding a new metric type without handling it elsewhere is a compile error.
 */
sealed trait Metric:
  def name: MetricName
  def labels: Labels
  def timestamp: Instant

/** An ever-increasing counter (e.g., events_ingested_total). */
final case class Counter(
    name: MetricName,
    value: Long,
    labels: Labels = Map.empty,
    timestamp: Instant = Instant.now()
) extends Metric

/** A point-in-time gauge (e.g., queue_depth, active_connections). */
final case class Gauge(
    name: MetricName,
    value: Double,
    labels: Labels = Map.empty,
    timestamp: Instant = Instant.now()
) extends Metric

/** A histogram summary with pre-computed quantiles. */
final case class Histogram(
    name: MetricName,
    count: Long,
    sum: Double,
    quantiles: Map[Double, Double],
    labels: Labels = Map.empty,
    timestamp: Instant = Instant.now()
) extends Metric

// ─────────────────────────────────────────────────────────────────────────────
// Companion object
// ─────────────────────────────────────────────────────────────────────────────

object Metric:
  /** Render a metric to a Prometheus text-format line. */
  def toPrometheusLine(m: Metric): String =
    val labelStr =
      if m.labels.isEmpty then ""
      else m.labels.map((k, v) => s"""$k="$v"""").mkString("{", ",", "}")
    m match
      case Counter(name, value, _, _)    => s"$name$labelStr $value"
      case Gauge(name, value, _, _)      => s"$name$labelStr $value"
      case Histogram(name, count, sum, quantiles, _, _) =>
        val qLines = quantiles.map { (q, v) => s"""${name}_bucket{quantile="$q"$labelStr} $v""" }
        (qLines ++ Seq(s"${name}_count $count", s"${name}_sum $sum")).mkString("\n")

// ─────────────────────────────────────────────────────────────────────────────
// MetricsCollector
// ─────────────────────────────────────────────────────────────────────────────

/**
 * Thread-safe metrics accumulator for the Atlas pipeline.
 *
 * NOTE: TrieMap gives us lock-free concurrent reads; LongAdder gives us
 * scalable concurrent increments — both are important at high ingestion rates.
 */
class MetricsCollector(val stageName: String):
  private val counters: TrieMap[MetricName, LongAdder] = TrieMap.empty
  private val gauges:   TrieMap[MetricName, Double]    = TrieMap.empty

  /** Increment a named counter by *delta*. */
  def increment(name: MetricName, delta: Long = 1L, labels: Labels = Map.empty): Unit =
    counters.getOrElseUpdate(labelledName(name, labels), new LongAdder).add(delta)

  /** Set a gauge to an absolute value. */
  def setGauge(name: MetricName, value: Double, labels: Labels = Map.empty): Unit =
    gauges.update(labelledName(name, labels), value)

  /** Snapshot all currently recorded metrics. */
  def snapshot(): Seq[Metric] =
    val cs = counters.map { (k, adder) => Counter(k, adder.sum()) }.toSeq
    val gs = gauges.map  { (k, v)     => Gauge(k, v) }.toSeq
    cs ++ gs

  /** Render all metrics in Prometheus exposition format. */
  def render(): String =
    snapshot().map(Metric.toPrometheusLine).mkString("\n")

  // HACK: embedding labels into the key string avoids a nested map but makes
  // the key non-round-trippable.  Switch to a proper (name, Labels) tuple key
  // once we need to support label cardinality queries (tracked in #501).
  private def labelledName(name: MetricName, labels: Labels): MetricName =
    if labels.isEmpty then name
    else name + "{" + labels.map((k, v) => s"$k=$v").mkString(",") + "}"

// ─────────────────────────────────────────────────────────────────────────────
// Companion factory
// ─────────────────────────────────────────────────────────────────────────────

object MetricsCollector:
  def forStage(stageName: String): MetricsCollector = new MetricsCollector(stageName)
