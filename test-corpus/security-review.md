# Security Review — Atlas Pipeline

**Reviewer:** External (Ironclad Security, contact: Alex Novak)
**Date:** March 20, 2026
**Requested by:** James Liu

## Findings

### Critical
1. **API keys in environment variables** — Atlas workers read Meridian Corp's API keys from Kubernetes secrets, but they're mounted as env vars. Recommendation: use HashiCorp Vault with dynamic secrets.

### High
2. **No rate limiting on ingestion endpoint** — An attacker could flood the pipeline. Sarah Chen acknowledged this is a known gap (tracked in ATL-189).
3. **S3 buckets not encrypted at rest** — The staging buckets in us-east-1 use default encryption. Should use customer-managed KMS keys.

### Medium
4. **Stale IAM roles** — Priya's audit found 3 unused IAM roles from the old deployment model. Should be cleaned up.
5. **No network policies in Kubernetes** — Pods can communicate freely. Should implement namespace-level network policies.

### Low
6. **Grafana accessible without VPN** — Currently IP-restricted but should require VPN for defense in depth.

## Timeline
James approved a 4-week remediation window. Sarah is leading the critical/high fixes, Priya handling infrastructure items.
