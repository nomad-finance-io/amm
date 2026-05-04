# infra

CDK stack that builds the Rust bot (in `../bot/`) into a Docker image, pushes it to ECR, and runs it as a single Fargate task.

## Configuration

All values come from env vars. The stack auto-loads `../.env` (repo root) on `cdk` invocation; existing shell env vars win over `.env` so you can override per-invocation.

| Var | Required | Source |
|---|---|---|
| `PYTH_CHANNEL` | yes | `.env` / shell |
| `PYTH_FEED_ID` | yes | `.env` / shell |
| `POOL_ID` | yes | `.env` / shell |
| `INTERVAL_MS` | yes | `.env` / shell |
| `HELIUS_API_KEY` | yes | Secrets Manager (name: `HELIUS_API_KEY_SECRET_NAME`, default `nomad/HELIUS_API_KEY`) |
| `PRIVATE_KEY` | yes | Secrets Manager (name: `PRIVATE_KEY_SECRET_NAME`, default `nomad/PRIVATE_KEY`) |
| `PYTH_LAZER_TOKEN` | yes | Secrets Manager (name: `PYTH_LAZER_TOKEN_SECRET_NAME`, default `nomad/PYTH_LAZER_TOKEN`) |
| `BOT_DIRECTORY` | no | defaults to repo root (Docker build context) |

Create the Secrets Manager secrets once before the first deploy:

```bash
aws secretsmanager create-secret --name nomad/HELIUS_API_KEY    --secret-string '...'
aws secretsmanager create-secret --name nomad/PRIVATE_KEY       --secret-string '...'
aws secretsmanager create-secret --name nomad/PYTH_LAZER_TOKEN  --secret-string '...'
```

## Deploy

Requires Docker running locally — CDK builds `../bot/Dockerfile` and pushes to the bootstrap ECR repo.

```bash
npm install
npx cdk bootstrap
npx cdk deploy
```

Override any value inline if needed:

```bash
INTERVAL_MS=500 npx cdk deploy
```

`CDK_DEFAULT_ACCOUNT` / `CDK_DEFAULT_REGION` are read from the active AWS profile.

## Synth-only (no Docker build, no AWS creds)

```bash
CDK_DOCKER=echo npx cdk synth
```
