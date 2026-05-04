#!/usr/bin/env tsx
import { Command, Option } from "commander";
import { PublicKey } from "@solana/web3.js";
import { CliConfig, CliFlags, resolveConfig } from "./config";
import {
  createAmmConfig,
  showAmmConfig,
  updateAmmConfig,
} from "./commands/amm-config";
import { closePermission, createPermission } from "./commands/permission";
import { setOracleKeeper, setPoolStatus } from "./commands/pool-admin";
import {
  CreatorFeeOnArg,
  OracleParams,
  createPool,
  createPoolPermissioned,
  showPool,
} from "./commands/pool";

const program = new Command();
program
  .name("nomad-cli")
  .description("Bootstrap and admin CLI for the Nomad Finance Oracle AMM program")
  .option(
    "-c, --config <path>",
    "client_config.ini override (otherwise read from solana CLI config + flags)",
  )
  .option(
    "--rpc-url <url>",
    "RPC endpoint override (default: solana CLI json_rpc_url, else mainnet)",
  )
  .option(
    "--keypair <path>",
    "payer/creator keypair (default: ~/.config/solana/id.json)",
  )
  .option(
    "--admin-keypair <path>",
    "program admin keypair for admin-only ops (default: same as --keypair)",
  )
  .option("--program-id <pubkey>", "override the program id baked into the IDL");

const cfg = (): CliConfig => resolveConfig(program.opts<CliFlags>());

const pk = (s: string) => new PublicKey(s);
const big = (s: string) => BigInt(s);

// ---------- amm-config ----------
const ammConfig = program.command("amm-config").description("AmmConfig admin ops");

ammConfig
  .command("create")
  .description("create a new AmmConfig PDA")
  .requiredOption("--index <u16>", "config index", parseInt)
  .requiredOption("--trade-fee-rate <u64>", "trade fee rate (1e-6 units)")
  .requiredOption("--protocol-fee-rate <u64>", "protocol fee rate (1e-6 units)")
  .requiredOption("--fund-fee-rate <u64>", "fund fee rate (1e-6 units)")
  .requiredOption("--create-pool-fee <u64>", "lamports charged on pool creation")
  .requiredOption("--creator-fee-rate <u64>", "creator fee rate (1e-6 units)")
  .action(async (opts) => {
    const r = await createAmmConfig(cfg(), {
      index: opts.index,
      tradeFeeRate: big(opts.tradeFeeRate),
      protocolFeeRate: big(opts.protocolFeeRate),
      fundFeeRate: big(opts.fundFeeRate),
      createPoolFee: big(opts.createPoolFee),
      creatorFeeRate: big(opts.creatorFeeRate),
    });
    console.log(`amm_config: ${r.ammConfig.toBase58()}`);
    console.log(`tx:         ${r.tx}`);
  });

ammConfig
  .command("update")
  .description("update an AmmConfig field by param id (see --help)")
  .requiredOption("--index <u16>", "config index", parseInt)
  .addOption(
    new Option(
      "--param <id>",
      "0=trade-fee 1=protocol-fee 2=fund-fee 3=protocol-owner 4=fund-owner 5=create-pool-fee 6=disable-create-pool 7=creator-fee",
    )
      .makeOptionMandatory()
      .argParser(parseInt),
  )
  .option("--value <u64>", "numeric value (ignored for params 3 and 4)", "0")
  .option("--new-key <pubkey>", "new owner pubkey (required for params 3 and 4)")
  .action(async (opts) => {
    const tx = await updateAmmConfig(cfg(), {
      index: opts.index,
      param: opts.param,
      value: big(opts.value),
      newKey: opts.newKey ? pk(opts.newKey) : undefined,
    });
    console.log(`tx: ${tx}`);
  });

ammConfig
  .command("show")
  .description("print an AmmConfig")
  .requiredOption("--index <u16>", "config index", parseInt)
  .action(async (opts) => showAmmConfig(cfg(), opts.index));

// ---------- permission ----------
const permission = program.command("permission").description("Permission PDA admin ops");

permission
  .command("create")
  .description("create permission PDA for an authority (gates initialize-with-permission)")
  .requiredOption("--authority <pubkey>", "authority allowed to create permissioned pools")
  .action(async (opts) => {
    const r = await createPermission(cfg(), pk(opts.authority));
    console.log(`permission: ${r.permission.toBase58()}`);
    console.log(`tx:         ${r.tx}`);
  });

permission
  .command("close")
  .description("close a permission PDA")
  .requiredOption("--authority <pubkey>", "authority whose permission PDA to close")
  .action(async (opts) => {
    const tx = await closePermission(cfg(), pk(opts.authority));
    console.log(`tx: ${tx}`);
  });

// ---------- pool ----------
const pool = program.command("pool").description("Pool create / inspect / admin");

const oracleOpts = (cmd: Command) =>
  cmd
    .option(
      "--pyth-feed-id <u32>",
      "Pyth Lazer feed id this pool prices against (must be non-zero)",
      "1",
    )
    .option("--min-spread-bps <u16>", "spread floor in bps (1..=1000)", "40")
    .option("--inventory-skew-enabled", "enable inventory-based quote skew", false)
    .option("--inventory-skew-deadzone-bps <u16>", "<=5000", "0")
    .option("--inventory-skew-bps-per-pct <u16>", "<=100", "0")
    .option("--inventory-skew-max-bps <u16>", "<=1000", "0")
    .requiredOption(
      "--oracle-keeper <pubkey>",
      "per-pool keeper bot pubkey allowed to push oracle updates",
    );

const oracleFromOpts = (opts: Record<string, string | boolean>): OracleParams => ({
  pythPriceFeedId: parseInt(opts.pythFeedId as string),
  minSpreadBps: parseInt(opts.minSpreadBps as string),
  inventorySkewEnabled: Boolean(opts.inventorySkewEnabled),
  inventorySkewDeadzoneBps: parseInt(opts.inventorySkewDeadzoneBps as string),
  inventorySkewBpsPerPct: parseInt(opts.inventorySkewBpsPerPct as string),
  inventorySkewMaxBps: parseInt(opts.inventorySkewMaxBps as string),
  oracleKeeper: pk(opts.oracleKeeper as string),
});

oracleOpts(
  pool
    .command("create")
    .description("create a pool (open permissionless creation)")
    .requiredOption("--config-index <u16>", "amm config index", parseInt)
    .requiredOption("--mint-a <pubkey>")
    .requiredOption("--mint-b <pubkey>")
    .requiredOption("--init-amount-a <u64>")
    .requiredOption("--init-amount-b <u64>")
    .option("--open-time <u64>", "unix-seconds when swaps may begin", "0"),
).action(async (opts) => {
  const r = await createPool(cfg(), {
    ammConfigIndex: opts.configIndex,
    mintA: pk(opts.mintA),
    mintB: pk(opts.mintB),
    initAmountA: big(opts.initAmountA),
    initAmountB: big(opts.initAmountB),
    openTime: big(opts.openTime),
    oracle: oracleFromOpts(opts),
  });
  console.log(`pool_state: ${r.poolState.toBase58()}`);
  console.log(`tx:         ${r.tx}`);
});

oracleOpts(
  pool
    .command("create-permissioned")
    .description("create a pool gated by an existing permission PDA")
    .requiredOption("--config-index <u16>", "amm config index", parseInt)
    .requiredOption("--mint-a <pubkey>")
    .requiredOption("--mint-b <pubkey>")
    .requiredOption("--init-amount-a <u64>")
    .requiredOption("--init-amount-b <u64>")
    .option("--open-time <u64>", "unix-seconds when swaps may begin", "0")
    .addOption(
      new Option("--creator-fee-on <variant>", "creator fee model")
        .choices(["both", "only0", "only1"] satisfies CreatorFeeOnArg[])
        .default("both"),
    ),
).action(async (opts) => {
  const r = await createPoolPermissioned(cfg(), {
    ammConfigIndex: opts.configIndex,
    mintA: pk(opts.mintA),
    mintB: pk(opts.mintB),
    initAmountA: big(opts.initAmountA),
    initAmountB: big(opts.initAmountB),
    openTime: big(opts.openTime),
    oracle: oracleFromOpts(opts),
    creatorFeeOn: opts.creatorFeeOn as CreatorFeeOnArg,
  });
  console.log(`pool_state: ${r.poolState.toBase58()}`);
  console.log(`tx:         ${r.tx}`);
});

pool
  .command("show")
  .description("print PoolState")
  .requiredOption("--pool-id <pubkey>")
  .action(async (opts) => showPool(cfg(), pk(opts.poolId)));

pool
  .command("set-status")
  .description("set the pool status byte (0=fully open)")
  .requiredOption("--pool-id <pubkey>")
  .requiredOption("--status <u8>", "0..255", parseInt)
  .action(async (opts) => {
    const tx = await setPoolStatus(cfg(), pk(opts.poolId), opts.status);
    console.log(`tx: ${tx}`);
  });

pool
  .command("set-oracle-keeper")
  .description("rotate a pool's oracle keeper signer")
  .requiredOption("--pool-id <pubkey>")
  .requiredOption("--new-keeper <pubkey>")
  .action(async (opts) => {
    const tx = await setOracleKeeper(cfg(), pk(opts.poolId), pk(opts.newKeeper));
    console.log(`tx: ${tx}`);
  });

program.parseAsync(process.argv).catch((err) => {
  console.error(err instanceof Error ? err.stack ?? err.message : err);
  process.exit(1);
});
