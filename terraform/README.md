# brainjar Terraform

This directory contains Terraform configuration to create the AWS infrastructure
needed by brainjar.

## Prerequisites

- [Terraform](https://terraform.io) >= 1.5
- [AWS CLI](https://aws.amazon.com/cli/) configured with appropriate credentials
- AWS account with Bedrock model access enabled for `amazon.titan-embed-text-v2:0`

## Usage

```bash
# Initialize providers
terraform init

# Review plan (set your bucket name)
terraform plan -var="s3_bucket_name=my-brainjar-bucket"

# Apply (creates all resources)
terraform apply -var="s3_bucket_name=my-brainjar-bucket"

# Copy outputs to brainjar.toml
terraform output brainjar_toml_snippet
```

## Resources Created

| Resource | Description |
|----------|-------------|
| `aws_s3_bucket` | Source documents for Bedrock ingestion |
| `awscc_s3vectors_vector_bucket` | S3 Vectors storage backend |
| `awscc_s3vectors_index` | Vector index with non-filterable metadata |
| `aws_bedrockagent_knowledge_base` | Bedrock KB with Titan Embed V2 |
| `aws_bedrockagent_data_source` | Connects S3 → KB (NONE chunking) |
| `aws_iam_role` | Service role for Bedrock to access S3 + S3 Vectors |

## Key Design Decisions

- **Chunking: NONE** — each file = one vector. This avoids metadata size issues
  (S3 Vectors has a 2KB filterable metadata limit with fixed-size chunking)
- **Non-filterable fields** — `AMAZON_BEDROCK_TEXT` and `AMAZON_BEDROCK_METADATA`
  are set as non-filterable in the vector index, which is required for Bedrock's
  internal metadata
- **Embedding model** — `amazon.titan-embed-text-v2:0` with dimension 1024

## Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `kb_name` | `memory` | Name used in resource naming |
| `region` | `us-east-1` | AWS region |
| `s3_bucket_name` | `brainjar-source-memory` | S3 bucket name |
| `embedding_model` | `amazon.titan-embed-text-v2:0` | Bedrock embedding model |
| `embedding_dimension` | `1024` | Vector dimension |
