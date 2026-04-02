# DevOps Runbook: Common Issues and Fixes

**Last Updated:** 2025-11-20  
**Maintainer:** Infrastructure Team

## Kuberentes Cluster Issues

### Pod Eviction Storms

When nodes run low on memory, kuberentes will start evicting pods aggressively. Check `kubectl describe node` for pressure indicators.

**Fix:** Scale down non-critical workloads, or add nodes to the cluster.

### CrashLoopBackoff

Common causes:
- Misconfigured archetecture (wrong env vars, missing secrets)
- Image pull failures (check registry credentials)
- Application startup failures (check logs with `kubectl logs`)

## Database Problems

### Postgress Connection Pool Exhaustion

Our PostgreSQL instances use pgbouncer for connection pooling. If you see "remaining connection slots are reserved" errors, the pool is saturated.

**Fix:** Increase `max_connections` in postgress config, or tune the application to release connections faster.

### ClickHouse Query Timeouts

clickHouse queries can timeout under heavy load. Check `system.query_log` for slow queries.

**Fix:** Add indexes on frequently-filtered columns, or increase `max_execution_time`.

## Terraform State Corruption

If `tf apply` fails with state lock errors, someone forgot to release the lock. You can force-unlock with:

```bash
tf force-unlock <lock-id>
```

**Warning:** Only do this if you're certain no one else is running tf commands.

## Kubernetes vs k8s

Use `k8s` as shorthand in docs, but spell out `kubernetes` in user-facing content.

## Postgres vs pg

Internal docs: `pg` is fine. External docs: `postgres` or `PostgreSQL`.

## Common Abbreviations

- **k8s** = Kubernetes
- **pg** = Postgres
- **tf** = Terraform  
- **gh** = GitHub
- **infra** = infrastructure
- **tf state** = Terraform state file

## Architecture vs Archetecture

Noticed several docs misspelling "architecture" as "archetecture". The correct spelling is **architecture**.

## PostgreSQL Casing

Officially: **PostgreSQL** (capital P, capital SQL)  
Acceptable: **Postgres** (informal)  
Wrong: **postgress**, **postgreSQL**, **postgres-sql**

## ClickHouse Casing

Officially: **ClickHouse** (capital C, capital H)  
Acceptable: **Clickhouse** (informal docs)  
Wrong: **clickhouse**, **click-house**, **clickHouse**
