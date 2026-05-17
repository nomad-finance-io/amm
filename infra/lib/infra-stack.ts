import * as path from 'path';
import * as cdk from 'aws-cdk-lib';
import { Construct } from 'constructs';
import * as ec2 from 'aws-cdk-lib/aws-ec2';
import * as ecs from 'aws-cdk-lib/aws-ecs';
import * as ecrAssets from 'aws-cdk-lib/aws-ecr-assets';
import * as logs from 'aws-cdk-lib/aws-logs';
import * as secretsmanager from 'aws-cdk-lib/aws-secretsmanager';

interface NomadStackProps extends cdk.StackProps {
  botDirectory: string;
  pythChannel: string;
  pythFeedId: string;
  poolId: string;
  intervalMs: string;
  solanaRpcUrlSecretName: string;
  privateKeySecretName: string;
  pythLazerTokenSecretName: string;
  cpu?: number;
  memoryMiB?: number;
}

export class InfraStack extends cdk.Stack {
  constructor(scope: Construct, id: string, props: NomadStackProps) {
    super(scope, id, props);

    const vpc = new ec2.Vpc(this, 'Vpc', {
      maxAzs: 2,
      natGateways: 1,
    });

    const cluster = new ecs.Cluster(this, 'Cluster', { vpc });

    const solanaRpcUrl = secretsmanager.Secret.fromSecretNameV2(
      this,
      'SolanaRpcUrl',
      props.solanaRpcUrlSecretName,
    );
    const privateKey = secretsmanager.Secret.fromSecretNameV2(
      this,
      'PrivateKeySecret',
      props.privateKeySecretName,
    );
    const pythLazerToken = secretsmanager.Secret.fromSecretNameV2(
      this,
      'PythLazerTokenSecret',
      props.pythLazerTokenSecretName,
    );

    const image = new ecrAssets.DockerImageAsset(this, 'BotImage', {
      // `botDirectory` is the repo root (Docker build context); the Dockerfile
      // sits inside `bot/`. Repo-root context is required so the bot's
      // path deps to `../client` and `../programs/cp-swap` resolve.
      directory: path.resolve(props.botDirectory),
      file: 'bot/Dockerfile',
      platform: ecrAssets.Platform.LINUX_AMD64,
    });

    const taskDefinition = new ecs.FargateTaskDefinition(this, 'TaskDef', {
      cpu: props.cpu ?? 512,
      memoryLimitMiB: props.memoryMiB ?? 1024,
      runtimePlatform: {
        cpuArchitecture: ecs.CpuArchitecture.X86_64,
        operatingSystemFamily: ecs.OperatingSystemFamily.LINUX,
      },
    });

    taskDefinition.addContainer('App', {
      image: ecs.ContainerImage.fromDockerImageAsset(image),
      logging: ecs.LogDrivers.awsLogs({
        streamPrefix: 'nomad',
        logRetention: logs.RetentionDays.ONE_MONTH,
      }),
      environment: {
        PYTH_CHANNEL: props.pythChannel,
        PYTH_FEED_ID: props.pythFeedId,
        POOL_ID: props.poolId,
        INTERVAL_MS: props.intervalMs,
      },
      secrets: {
        SOLANA_RPC_URL: ecs.Secret.fromSecretsManager(solanaRpcUrl),
        PRIVATE_KEY: ecs.Secret.fromSecretsManager(privateKey),
        PYTH_LAZER_TOKEN: ecs.Secret.fromSecretsManager(pythLazerToken),
      },
    });

    const service = new ecs.FargateService(this, 'Service', {
      cluster,
      taskDefinition,
      desiredCount: 1,
      assignPublicIp: false,
      circuitBreaker: { rollback: true },
      minHealthyPercent: 0,
      maxHealthyPercent: 100,
    });

    new cdk.CfnOutput(this, 'ClusterName', { value: cluster.clusterName });
    new cdk.CfnOutput(this, 'ServiceName', { value: service.serviceName });
    new cdk.CfnOutput(this, 'TaskDefinitionArn', {
      value: taskDefinition.taskDefinitionArn,
    });
  }
}
