terraform {
  required_version = ">= 1.5"
  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 5.0"
    }
    awscc = {
      source  = "hashicorp/awscc"
      version = "~> 1.0"
    }
  }
}

provider "aws" {
  region = var.region
}

provider "awscc" {
  region = var.region
}

# ── S3 bucket for source documents ───────────────────────────────────────────

resource "aws_s3_bucket" "brainjar_source" {
  bucket = var.s3_bucket_name

  tags = {
    Project = "brainjar"
    KB      = var.kb_name
  }
}

resource "aws_s3_bucket_versioning" "source" {
  bucket = aws_s3_bucket.brainjar_source.id
  versioning_configuration {
    status = "Enabled"
  }
}

resource "aws_s3_bucket_server_side_encryption_configuration" "source" {
  bucket = aws_s3_bucket.brainjar_source.id
  rule {
    apply_server_side_encryption_by_default {
      sse_algorithm = "AES256"
    }
  }
}

resource "aws_s3_bucket_public_access_block" "source" {
  bucket                  = aws_s3_bucket.brainjar_source.id
  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

# ── IAM role for Bedrock ──────────────────────────────────────────────────────

resource "aws_iam_role" "bedrock_kb" {
  name = "brainjar-bedrock-kb-${var.kb_name}"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect = "Allow"
      Principal = {
        Service = "bedrock.amazonaws.com"
      }
      Action = "sts:AssumeRole"
      Condition = {
        StringEquals = {
          "aws:SourceAccount" = data.aws_caller_identity.current.account_id
        }
        ArnLike = {
          "aws:SourceArn" = "arn:aws:bedrock:${var.region}:${data.aws_caller_identity.current.account_id}:knowledge-base/*"
        }
      }
    }]
  })
}

data "aws_caller_identity" "current" {}

resource "aws_iam_role_policy" "bedrock_kb_s3" {
  name = "brainjar-s3-access"
  role = aws_iam_role.bedrock_kb.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Effect = "Allow"
        Action = [
          "s3:GetObject",
          "s3:ListBucket",
          "s3:PutObject",
          "s3:DeleteObject"
        ]
        Resource = [
          aws_s3_bucket.brainjar_source.arn,
          "${aws_s3_bucket.brainjar_source.arn}/*"
        ]
      }
    ]
  })
}

resource "aws_iam_role_policy" "bedrock_kb_model" {
  name = "brainjar-bedrock-model"
  role = aws_iam_role.bedrock_kb.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Effect   = "Allow"
        Action   = ["bedrock:InvokeModel"]
        Resource = "arn:aws:bedrock:${var.region}::foundation-model/${var.embedding_model}"
      }
    ]
  })
}

resource "aws_iam_role_policy" "bedrock_kb_vectors" {
  name = "brainjar-s3vectors-access"
  role = aws_iam_role.bedrock_kb.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Effect = "Allow"
        Action = [
          "s3vectors:*"
        ]
        Resource = "*"
      }
    ]
  })
}

# ── S3 Vectors index (via awscc) ──────────────────────────────────────────────

resource "awscc_s3vectors_vector_bucket" "brainjar" {
  vector_bucket_name = "brainjar-vectors-${var.kb_name}"
}

resource "awscc_s3vectors_index" "brainjar" {
  vector_bucket_name = awscc_s3vectors_vector_bucket.brainjar.vector_bucket_name
  index_name         = "brainjar-index-${var.kb_name}"

  data_type        = "float32"
  dimension        = var.embedding_dimension
  distance_metrics = "cosine"

  # Non-filterable metadata fields required by Bedrock
  metadata_configuration = {
    non_filterable_metadata_keys = [
      "AMAZON_BEDROCK_TEXT",
      "AMAZON_BEDROCK_METADATA"
    ]
  }
}

# ── Bedrock Knowledge Base ────────────────────────────────────────────────────

resource "aws_bedrockagent_knowledge_base" "brainjar" {
  name     = "brainjar-${var.kb_name}"
  role_arn = aws_iam_role.bedrock_kb.arn

  knowledge_base_configuration {
    type = "VECTOR"
    vector_knowledge_base_configuration {
      embedding_model_arn = "arn:aws:bedrock:${var.region}::foundation-model/${var.embedding_model}"
    }
  }

  storage_configuration {
    type = "S3_VECTORS"
    s3_vectors_configuration {
      vector_bucket_arn = "arn:aws:s3:${var.region}:${data.aws_caller_identity.current.account_id}:bucket/${awscc_s3vectors_vector_bucket.brainjar.vector_bucket_name}"
      index_arn         = awscc_s3vectors_index.brainjar.index_arn
    }
  }

  tags = {
    Project = "brainjar"
    KB      = var.kb_name
  }
}

# ── Bedrock Data Source ───────────────────────────────────────────────────────

resource "aws_bedrockagent_data_source" "brainjar" {
  knowledge_base_id = aws_bedrockagent_knowledge_base.brainjar.id
  name              = "brainjar-source-${var.kb_name}"

  data_source_configuration {
    type = "S3"
    s3_configuration {
      bucket_arn = aws_s3_bucket.brainjar_source.arn
    }
  }

  vector_ingestion_configuration {
    chunking_configuration {
      chunking_strategy = "NONE"
    }
  }
}
