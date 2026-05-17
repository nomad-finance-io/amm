#!/usr/bin/env node
import * as fs from 'fs';
import * as path from 'path';
import * as cdk from 'aws-cdk-lib';
import { InfraStack } from '../lib/infra-stack';

// Repo root is the Docker build context. The bot's Cargo.toml has path deps
// to `../client` and `../programs/cp-swap`, so the build context must be
// wide enough to see them. The Dockerfile itself lives at `bot/Dockerfile`
// (set via `file:` in the DockerImageAsset).
const repoRoot = path.resolve(__dirname, '..', '..');

loadEnvFile(path.join(repoRoot, '.env'));

const app = new cdk.App();

new InfraStack(app, 'NomadEcsStack', {
  env: {
    account: process.env.CDK_DEFAULT_ACCOUNT,
    region: process.env.CDK_DEFAULT_REGION,
  },
  botDirectory: process.env.BOT_DIRECTORY ?? repoRoot,
  pythChannel: requireEnv('PYTH_CHANNEL'),
  pythFeedId: requireEnv('PYTH_FEED_ID'),
  poolId: requireEnv('POOL_ID'),
  intervalMs: requireEnv('INTERVAL_MS'),
  solanaRpcUrlSecretName:
    process.env.SOLANA_RPC_URL_SECRET_NAME ?? 'nomad/SOLANA_RPC_URL',
  privateKeySecretName:
    process.env.PRIVATE_KEY_SECRET_NAME ?? 'nomad/PRIVATE_KEY',
  pythLazerTokenSecretName:
    process.env.PYTH_LAZER_TOKEN_SECRET_NAME ?? 'nomad/PYTH_LAZER_TOKEN',
});

function requireEnv(key: string): string {
  const value = process.env[key];
  if (typeof value !== 'string' || value.length === 0) {
    throw new Error(`Missing required env var: ${key} (set in .env or shell)`);
  }
  return value;
}

// Existing process env wins over .env so callers can override per-invocation.
function loadEnvFile(filePath: string): void {
  if (!fs.existsSync(filePath)) return;
  const contents = fs.readFileSync(filePath, 'utf8');
  for (const rawLine of contents.split('\n')) {
    const line = rawLine.trim();
    if (line.length === 0 || line.startsWith('#')) continue;
    const eq = line.indexOf('=');
    if (eq === -1) continue;
    const key = line.slice(0, eq).trim();
    if (key.length === 0 || process.env[key] !== undefined) continue;
    let value = line.slice(eq + 1).trim();
    if (
      (value.startsWith('"') && value.endsWith('"')) ||
      (value.startsWith("'") && value.endsWith("'"))
    ) {
      value = value.slice(1, -1);
    }
    process.env[key] = value;
  }
}
