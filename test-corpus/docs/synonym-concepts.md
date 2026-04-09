# System Design: Lookup Acceleration Layer

**Author:** Engineering Team  
**Date:** 2025-11-10

## Overview

Our API response times degraded as database query volume increased. Repeated requests for identical data were hitting the database unnecessarily, driving up latency and cost.

## Solution: Memoization Infrastructure

We implemented a hot data store that sits between the application layer and PostgreSQL. When a request arrives, the system checks the lookup acceleration layer first. If the data exists there, we return it immediately (sub-millisecond response time). If not, we query the database, store the result in the hot layer, and return it to the client.

This memoization strategy reduced our P95 latency from 450ms to 78ms for frequently-accessed endpoints.

## Implementation Details

**Storage backend:** Redis cluster (3 nodes, replication factor 2)  
**TTL policy:** 5 minutes for user profiles, 30 seconds for real-time data  
**Eviction:** LRU when memory exceeds 80% capacity

The hot data store holds approximately 500K keys at steady state, consuming 4GB of memory across the cluster.

## Identity Verification Flow

Our credential validation system was previously synchronous — each login request triggered an LDAP query, adding 200-300ms to the authentication flow.

We now pre-populate the lookup acceleration layer with user credentials during off-peak hours (3 AM daily sync). When a user logs in, we verify their identity against the hot store instead of hitting LDAP. This cut login time from 1.2 seconds to 340ms.

For high-security operations (password changes, 2FA setup), we still hit LDAP directly to ensure freshness.

## Access Control Matrix

The authorization system previously queried the permissions table on every request. With 50K users and 200 resources, this meant millions of permission checks per day.

We moved the access control matrix into the hot layer. Each user's permissions are cached for 10 minutes. When permissions change (role update, resource reassignment), we invalidate the relevant cache entries immediately.

This approach balances freshness (10-minute staleness window) with performance (90% cache hit rate).

## Monitoring

We track:
- **Hit ratio:** Percentage of requests served from the hot layer (target: >85%)
- **Eviction rate:** How often we're kicking data out due to memory pressure (target: <5% of keys/hour)
- **Replication lag:** Time between write and replica acknowledgment (target: <50ms)

If hit ratio drops below 75%, it indicates our TTL policies are too aggressive or traffic patterns have shifted. If eviction rate spikes, we need to add memory capacity.

## Cost Impact

**Before:** Database CPU utilization at 72%, approaching scale-up threshold  
**After:** Database CPU dropped to 31%, delaying the need for a larger instance by 6+ months  
**Savings:** ~$800/month in deferred database costs, offset by $120/month for the hot store cluster  
**Net benefit:** $680/month, or $8,160/year

## Future Work

We're evaluating a second tier of memoization for infrequently-accessed data (hourly batch jobs, analytics queries). This would use disk-backed storage (SSD) instead of in-memory, trading latency for cost efficiency on cold data.
