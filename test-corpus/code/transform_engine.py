"""
transform_engine.py -- Atlas transformation pipeline.

Applies AtlasQL transformation rules to raw IngestEvents, producing
typed TransformResults ready for storage in ClickHouse.
"""

from __future__ import annotations

import logging
import time
from dataclasses import dataclass, field
from functools import wraps
from typing import Any, Callable, Iterator, TypeAlias

logger = logging.getLogger("atlas.transform")

# Type alias for the raw event payload coming out of the ingestion layer.
RawPayload: TypeAlias = dict[str, Any]

# Maximum number of transformation retries before a record is sent to the DLQ.
MAX_RETRIES: int = 3


def timed(fn: Callable) -> Callable:
    """Decorator that logs execution time of any transform step."""

    @wraps(fn)
    def wrapper(*args, **kwargs):
        start = time.perf_counter()
        result = fn(*args, **kwargs)
        elapsed_ms = (time.perf_counter() - start) * 1000
        logger.debug("%s completed in %.2f ms", fn.__name__, elapsed_ms)
        return result

    return wrapper


@dataclass
class TransformRule:
    """A single AtlasQL transformation rule."""

    name: str
    expression: str
    priority: int = 0
    # WHY: enabled flag lets operators hot-toggle rules without redeployment.
    enabled: bool = True


@dataclass
class TransformResult:
    """Outcome of applying transformation rules to one event."""

    event_id: str
    source: str
    transformed: dict[str, Any]
    rules_applied: list[str] = field(default_factory=list)
    error: str | None = None

    @property
    def success(self) -> bool:
        return self.error is None


class RuleRegistry:
    """Maintains the ordered set of AtlasQL rules for a given pipeline stage."""

    def __init__(self) -> None:
        self._rules: list[TransformRule] = []

    def register(self, rule: TransformRule) -> None:
        """Add a rule, keeping the registry sorted by priority (ascending)."""
        self._rules.append(rule)
        self._rules.sort(key=lambda r: r.priority)

    def active_rules(self) -> Iterator[TransformRule]:
        """Yield only the enabled rules in priority order."""
        yield from (r for r in self._rules if r.enabled)


class TransformEngine:
    """
    Core transformation engine.

    Pulls events from the ingestion queue, applies registered rules in order,
    and emits TransformResults to the storage layer.
    """

    def __init__(self, registry: RuleRegistry, batch_size: int = 100) -> None:
        self.registry = registry
        self.batch_size = batch_size
        self._processed = 0
        self._errors = 0

    @timed
    def _apply_rules(self, payload: RawPayload, event_id: str, source: str) -> TransformResult:
        """
        Apply all active rules to *payload*.

        NOTE: Rules are applied sequentially; each rule sees the output of the
        previous one, so ordering in the registry matters.
        """
        current = dict(payload)
        applied: list[str] = []

        for rule in self.registry.active_rules():
            try:
                # Evaluate the AtlasQL expression (stub: real eval uses atlas_ql crate via FFI)
                current = self._eval_expression(rule.expression, current)
                applied.append(rule.name)
            except Exception as exc:  # noqa: BLE001
                return TransformResult(
                    event_id=event_id,
                    source=source,
                    transformed=current,
                    rules_applied=applied,
                    error=str(exc),
                )

        return TransformResult(
            event_id=event_id,
            source=source,
            transformed=current,
            rules_applied=applied,
        )

    def _eval_expression(self, expression: str, data: RawPayload) -> RawPayload:
        """Stub expression evaluator — replace with FFI call in production."""
        # HACK: identity transform used during testing; real implementation calls
        # into the Rust AtlasQL evaluator via cffi bindings (see atlas_ql/bindings.py).
        _ = expression
        return data

    def process_batch(self, events: list[tuple[str, str, RawPayload]]) -> list[TransformResult]:
        """
        Process a batch of (event_id, source, payload) tuples.

        Returns one TransformResult per input event.
        """
        results: list[TransformResult] = []
        for event_id, source, payload in events:
            result = self._apply_rules(payload, event_id, source)
            if result.success:
                self._processed += 1
            else:
                self._errors += 1
                logger.warning("transform error for event %s: %s", event_id, result.error)
            results.append(result)
        return results

    @property
    def stats(self) -> dict[str, int]:
        """Return a snapshot of processing counters."""
        return {"processed": self._processed, "errors": self._errors}
