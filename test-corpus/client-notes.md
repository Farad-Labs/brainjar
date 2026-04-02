# Client Notes

## Meridian Corp
- **Primary contact:** Rachel Torres (VP Engineering)
- **Contract:** $45K/month, renewed annually (next renewal: September 2026)
- **Integration:** REST API v3 (source of the March 10 incident)
- **Pain points:** Want real-time dashboards (currently batch, 15-min delay)
- Diana Ross manages the relationship day-to-day
- Rachel mentioned they're also evaluating Snowflake — we need to show Atlas is competitive

## Beacon Health
- **Primary contact:** Dr. Omar Hassan (Chief Data Officer)
- **Status:** Evaluating, demo completed March 25
- **Requirements:** HIPAA compliance (SOC 2 is a prerequisite), HL7 FHIR data format support
- **Estimated revenue:** $30K/month
- James Liu is leading the sales process
- Need to resolve: can AtlasQL handle FHIR resources? Marcus thinks yes with minor extensions

## TerraForm Energy
- **Primary contact:** Lisa Park (Head of Analytics)
- **Status:** Signed letter of intent, contract negotiation in progress
- **Requirements:** IoT sensor data at 100K events/second (higher than our current capacity)
- **Estimated revenue:** $25K/month
- Sarah thinks the ingestion layer can handle it with horizontal scaling
- Priya estimates we'd need 3 additional EKS nodes ($2,400/month infrastructure cost)
