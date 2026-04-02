# Onboarding Guide

Welcome to Nexus Labs! Here's what you need to get started.

## Day 1
1. Get your AWS IAM credentials from Priya Patel
2. Clone the Atlas monorepo: `git clone git@github.com:nexus-labs/atlas.git`
3. Install Rust (1.78+), Docker, and kubectl
4. Run `make setup` to configure local development

## Development Workflow
- Branch from `main`, PR into `staging`, then promote to `production`
- All PRs require approval from Sarah Chen or Marcus Webb
- CI runs on GitHub Actions: lint, test, integration test (~8 min)
- Deploy via ArgoCD — Priya manages the deployment configs

## Key Contacts
| Person | Role | Slack |
|--------|------|-------|
| Sarah Chen | Tech Lead | @sarah.chen |
| Marcus Webb | Backend | @marcus.webb |
| Priya Patel | DevOps | @priya.patel |
| James Liu | CTO | @james.liu |
| Diana Ross | Product Manager | @diana.ross |

## Important Links
- Grafana: https://grafana.nexuslabs.internal
- ArgoCD: https://argo.nexuslabs.internal
- Confluence: https://nexuslabs.atlassian.net
