# Project Helix: Compliance & Governance

**Project Lead:** Dr. Yuki Tanaka  
**Status:** Active  
**Last Updated:** 2025-12-18

## Overview

Project Helix is Nexus Labs' next-generation data governance framework, designed to enforce data residency, retention, and access policies across all internal systems.

**Scope:** All production databases, data lakes, and API endpoints  
**Timeline:** 18 months (Jan 2026 - Jun 2027)  
**Budget:** $2.4M (approved by executive committee)

## Organizational Context

Dr. Yuki Tanaka was appointed as Project Lead in November 2025. Yuki reports directly to **James Liu** (VP of Engineering), who sponsors the initiative at the executive level.

James secured board approval for the budget in October 2025, emphasizing the strategic importance of data governance in light of expanding regulatory requirements (GDPR, CCPA, HIPAA).

## Compliance Objectives

Project Helix aims to achieve:
- **SOC 2 Type II certification** (target: Q2 2026)
- **ISO 27001 compliance** (target: Q4 2026)
- **GDPR data subject request automation** (target: Q1 2027)

### SOC 2 Audit

James Liu is the executive sponsor for the **SOC 2 audit**, which is a prerequisite for several enterprise customer contracts. The audit covers:
- Access controls (identity verification, authorization)
- Data encryption (at rest and in transit)
- Incident response procedures
- Vendor management

The audit is scheduled for Q2 2026, with PwC as the auditing firm. James secured their engagement in November 2025.

### Compliance Team

**Audit lead:** External (PwC)  
**Internal coordinator:** James Liu  
**Technical implementation:** Dr. Yuki Tanaka (Project Helix)  
**Policy review:** Legal (Sarah Chen liaison)

## Data Residency Requirements

Project Helix enforces geographic data residency rules:
- **EU customer data:** Frankfurt region (AWS eu-central-1)
- **US customer data:** Virginia region (AWS us-east-1)
- **APAC customer data:** Singapore region (AWS ap-southeast-1)

Cross-region replication is disabled by default. Any cross-border data transfer requires explicit customer consent, logged in the governance audit trail.

## Technical Architecture

### Policy Engine

The policy engine evaluates data access requests against the compliance ruleset:
1. User requests data via API
2. Policy engine checks: geography, retention period, access level
3. If approved: data returned (access logged)
4. If denied: 403 response (denial logged)

All policy decisions are immutable logs, retained for 7 years per SOC 2 requirements.

### Retention Automation

Project Helix automates data deletion based on retention policies:
- **User data:** 90 days post-account-closure (GDPR Article 17)
- **Audit logs:** 7 years (SOC 2 requirement)
- **Analytics data:** 13 months (internal policy)

Deletion jobs run nightly, with verification checks to ensure compliance.

## Risks & Mitigations

**Risk:** Scope creep due to evolving regulations  
**Mitigation:** Quarterly policy review with Legal, James Liu as escalation path

**Risk:** Technical complexity delays SOC 2 audit readiness  
**Mitigation:** Dr. Yuki Tanaka has checkpoint reviews with James every 2 weeks

**Risk:** Vendor delays (PwC availability)  
**Mitigation:** James secured Q2 2026 slot in November, contract signed

## Success Metrics

- SOC 2 certification achieved by Q2 2026
- Zero data residency policy violations in production
- 100% retention policy compliance (automated verification)
- Executive sponsorship maintained (James Liu quarterly reviews)

## Key Relationships

- **Project Helix → Dr. Yuki Tanaka** (project lead)
- **Dr. Yuki Tanaka → James Liu** (reporting, executive sponsor)
- **James Liu → SOC 2 audit** (executive sponsor, vendor management)
- **SOC 2 audit → Compliance objectives** (certification milestone)

## Next Steps

1. Complete policy engine MVP (Jan 2026)
2. Begin PwC pre-audit review (Feb 2026)
3. Remediate findings from pre-audit (Mar-Apr 2026)
4. SOC 2 Type II audit (May-Jun 2026)
5. Certification issued (Jul 2026)
