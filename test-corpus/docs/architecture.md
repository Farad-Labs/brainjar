# Atlas Architecture

## Ingestion Layer
The ingestion layer accepts events via HTTP (REST and gRPC) and Kafka consumers. Sarah Chen designed this layer to handle 50K events/second with backpressure.

Events flow through a validation pipeline before landing in the staging area (S3 buckets in us-east-1).

## Transformation Engine
Marcus Webb's transformation engine reads from S3, applies user-defined transforms (written in a custom DSL called AtlasQL), and writes results to the output store.

The engine runs as a set of Kubernetes jobs managed by Argo Workflows. Each job processes a single partition (~100K events).

## Storage
- **Hot storage:** ClickHouse (proposed, currently PostgreSQL)
- **Cold storage:** S3 Glacier
- **Metadata:** PostgreSQL on RDS

## Monitoring
We use Grafana + Prometheus for metrics, with PagerDuty alerts for pipeline failures. The on-call rotation is: Marcus (week 1), Priya (week 2), Sarah (week 3).
