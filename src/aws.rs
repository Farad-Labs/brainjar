use anyhow::Result;
use aws_config::{BehaviorVersion, Region};
use aws_sdk_bedrockagent::Client as BedrockAgentClient;
use aws_sdk_bedrockagentruntime::Client as BedrockRuntimeClient;
use aws_sdk_s3::Client as S3Client;

use crate::config::AwsConfig;

pub struct AwsClients {
    pub s3: S3Client,
    pub bedrock_agent: BedrockAgentClient,
    pub bedrock_runtime: BedrockRuntimeClient,
}

pub async fn build_clients(aws_config: &AwsConfig) -> Result<AwsClients> {
    let mut builder = aws_config::defaults(BehaviorVersion::latest());

    if let Some(profile) = &aws_config.profile {
        builder = builder.profile_name(profile);
    }

    if let Some(region) = &aws_config.region {
        builder = builder.region(Region::new(region.clone()));
    }

    let config = builder.load().await;

    Ok(AwsClients {
        s3: S3Client::new(&config),
        bedrock_agent: BedrockAgentClient::new(&config),
        bedrock_runtime: BedrockRuntimeClient::new(&config),
    })
}
