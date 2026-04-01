output "kb_id" {
  description = "Bedrock Knowledge Base ID — add to brainjar.toml"
  value       = aws_bedrockagent_knowledge_base.brainjar.id
}

output "data_source_id" {
  description = "Bedrock Data Source ID — add to brainjar.toml"
  value       = aws_bedrockagent_data_source.brainjar.data_source_id
}

output "s3_bucket" {
  description = "S3 bucket name — add to brainjar.toml"
  value       = aws_s3_bucket.brainjar_source.bucket
}

output "vector_bucket_name" {
  description = "S3 Vectors bucket name"
  value       = awscc_s3vectors_vector_bucket.brainjar.vector_bucket_name
}

output "bedrock_role_arn" {
  description = "IAM role ARN used by Bedrock"
  value       = aws_iam_role.bedrock_kb.arn
}

output "brainjar_toml_snippet" {
  description = "Copy this into brainjar.toml"
  value       = <<-EOT
    [knowledge_bases.${var.kb_name}]
    kb_id = "${aws_bedrockagent_knowledge_base.brainjar.id}"
    data_source_id = "${aws_bedrockagent_data_source.brainjar.data_source_id}"
    s3_bucket = "${aws_s3_bucket.brainjar_source.bucket}"
    watch_paths = ["memory/", "MEMORY.md"]
    auto_sync = true
  EOT
}
