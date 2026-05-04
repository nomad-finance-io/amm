import { BN } from "@coral-xyz/anchor";
import {
  PublicKey,
  SystemProgram,
  SYSVAR_RENT_PUBKEY,
} from "@solana/web3.js";
import {
  ASSOCIATED_TOKEN_PROGRAM_ID,
  TOKEN_PROGRAM_ID,
  getAssociatedTokenAddressSync,
} from "@solana/spl-token";
import { CliConfig, makeConnection } from "../config";
import { buildProgram, loadPayer } from "../program";
import {
  findAmmConfig,
  findAuthority,
  findLpMint,
  findObservation,
  findPermission,
  findPool,
  findPoolVault,
} from "../pdas";

const CREATE_POOL_FEE_RECEIVER = new PublicKey(
  "9WNaCaNpU85yUCq5LrvpLKyucA4jAT3up1KJD1P2G34C",
);

export interface OracleParams {
  pythPriceFeedId: number;
  minSpreadBps: number;
  inventorySkewEnabled: boolean;
  inventorySkewDeadzoneBps: number;
  inventorySkewBpsPerPct: number;
  inventorySkewMaxBps: number;
  oracleKeeper: PublicKey;
}

interface PoolInitInputs {
  ammConfigIndex: number;
  mintA: PublicKey;
  mintB: PublicKey;
  initAmountA: bigint;
  initAmountB: bigint;
  openTime: bigint;
  oracle: OracleParams;
}

interface ResolvedPool {
  ammConfig: PublicKey;
  authority: PublicKey;
  poolState: PublicKey;
  token0: PublicKey;
  token1: PublicKey;
  initAmount0: BN;
  initAmount1: BN;
  token0Program: PublicKey;
  token1Program: PublicKey;
  lpMint: PublicKey;
  vault0: PublicKey;
  vault1: PublicKey;
  observation: PublicKey;
}

/**
 * Resolve mint ordering, fetch each mint's owning token program (SPL or
 * Token-2022), and derive every PDA the pool init touches.
 */
async function resolvePool(cfg: CliConfig, inputs: PoolInitInputs): Promise<ResolvedPool> {
  const conn = makeConnection(cfg);
  const ammConfig = findAmmConfig(inputs.ammConfigIndex, cfg.programId);
  const authority = findAuthority(cfg.programId);

  // The on-chain `Initialize` account constraint requires token_0 < token_1.
  const [token0, token1, initAmount0, initAmount1] = inputs.mintA.toBuffer().compare(
    inputs.mintB.toBuffer(),
  ) < 0
    ? [inputs.mintA, inputs.mintB, inputs.initAmountA, inputs.initAmountB]
    : [inputs.mintB, inputs.mintA, inputs.initAmountB, inputs.initAmountA];

  const [acc0, acc1] = await conn.getMultipleAccountsInfo([token0, token1]);
  if (!acc0) throw new Error(`mint not found: ${token0.toBase58()}`);
  if (!acc1) throw new Error(`mint not found: ${token1.toBase58()}`);

  const poolState = findPool(ammConfig, token0, token1, cfg.programId);

  return {
    ammConfig,
    authority,
    poolState,
    token0,
    token1,
    initAmount0: new BN(initAmount0.toString()),
    initAmount1: new BN(initAmount1.toString()),
    token0Program: acc0.owner,
    token1Program: acc1.owner,
    lpMint: findLpMint(poolState, cfg.programId),
    vault0: findPoolVault(poolState, token0, cfg.programId),
    vault1: findPoolVault(poolState, token1, cfg.programId),
    observation: findObservation(poolState, cfg.programId),
  };
}

export async function createPool(
  cfg: CliConfig,
  inputs: PoolInitInputs,
): Promise<{ poolState: PublicKey; tx: string }> {
  const payer = loadPayer(cfg);
  const { program } = buildProgram(cfg, payer);
  const r = await resolvePool(cfg, inputs);
  const o = inputs.oracle;

  const creatorToken0 = getAssociatedTokenAddressSync(
    r.token0,
    payer.publicKey,
    false,
    r.token0Program,
  );
  const creatorToken1 = getAssociatedTokenAddressSync(
    r.token1,
    payer.publicKey,
    false,
    r.token1Program,
  );
  const creatorLp = getAssociatedTokenAddressSync(
    r.lpMint,
    payer.publicKey,
    false,
    TOKEN_PROGRAM_ID,
  );

  const tx = await program.methods
    .initialize(
      r.initAmount0,
      r.initAmount1,
      new BN(inputs.openTime.toString()),
      o.pythPriceFeedId,
      o.minSpreadBps,
      o.inventorySkewEnabled,
      o.inventorySkewDeadzoneBps,
      o.inventorySkewBpsPerPct,
      o.inventorySkewMaxBps,
      o.oracleKeeper,
    )
    .accounts({
      creator: payer.publicKey,
      ammConfig: r.ammConfig,
      authority: r.authority,
      poolState: r.poolState,
      token0Mint: r.token0,
      token1Mint: r.token1,
      lpMint: r.lpMint,
      creatorToken0,
      creatorToken1,
      creatorLpToken: creatorLp,
      token0Vault: r.vault0,
      token1Vault: r.vault1,
      createPoolFee: CREATE_POOL_FEE_RECEIVER,
      observationState: r.observation,
      tokenProgram: TOKEN_PROGRAM_ID,
      token0Program: r.token0Program,
      token1Program: r.token1Program,
      associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
      systemProgram: SystemProgram.programId,
      rent: SYSVAR_RENT_PUBKEY,
    })
    .rpc();
  return { poolState: r.poolState, tx };
}

export type CreatorFeeOnArg = "both" | "only0" | "only1";

const CREATOR_FEE_ON_VARIANT: Record<CreatorFeeOnArg, Record<string, never>> = {
  both: {} as never,
  only0: {} as never,
  only1: {} as never,
};

function creatorFeeOnPayload(arg: CreatorFeeOnArg) {
  // Anchor encodes Rust-style enums as `{ variantName: {} }`.
  switch (arg) {
    case "both":
      return { bothToken: {} };
    case "only0":
      return { onlyToken0: {} };
    case "only1":
      return { onlyToken1: {} };
  }
}

export async function createPoolPermissioned(
  cfg: CliConfig,
  inputs: PoolInitInputs & { creatorFeeOn: CreatorFeeOnArg },
): Promise<{ poolState: PublicKey; tx: string }> {
  // Touch the variant table so unused-variable lints stay quiet if `CreatorFeeOnArg` is imported elsewhere later.
  void CREATOR_FEE_ON_VARIANT;

  const payer = loadPayer(cfg);
  const { program } = buildProgram(cfg, payer);
  const r = await resolvePool(cfg, inputs);
  const o = inputs.oracle;
  const permission = findPermission(payer.publicKey, cfg.programId);

  const payerToken0 = getAssociatedTokenAddressSync(
    r.token0,
    payer.publicKey,
    false,
    r.token0Program,
  );
  const payerToken1 = getAssociatedTokenAddressSync(
    r.token1,
    payer.publicKey,
    false,
    r.token1Program,
  );
  const payerLp = getAssociatedTokenAddressSync(
    r.lpMint,
    payer.publicKey,
    false,
    TOKEN_PROGRAM_ID,
  );

  const tx = await program.methods
    .initializeWithPermission(
      r.initAmount0,
      r.initAmount1,
      new BN(inputs.openTime.toString()),
      creatorFeeOnPayload(inputs.creatorFeeOn) as never,
      o.pythPriceFeedId,
      o.minSpreadBps,
      o.inventorySkewEnabled,
      o.inventorySkewDeadzoneBps,
      o.inventorySkewBpsPerPct,
      o.inventorySkewMaxBps,
      o.oracleKeeper,
    )
    .accounts({
      payer: payer.publicKey,
      creator: payer.publicKey,
      ammConfig: r.ammConfig,
      authority: r.authority,
      poolState: r.poolState,
      token0Mint: r.token0,
      token1Mint: r.token1,
      lpMint: r.lpMint,
      payerToken0,
      payerToken1,
      payerLpToken: payerLp,
      token0Vault: r.vault0,
      token1Vault: r.vault1,
      createPoolFee: CREATE_POOL_FEE_RECEIVER,
      observationState: r.observation,
      permission,
      tokenProgram: TOKEN_PROGRAM_ID,
      token0Program: r.token0Program,
      token1Program: r.token1Program,
      associatedTokenProgram: ASSOCIATED_TOKEN_PROGRAM_ID,
      systemProgram: SystemProgram.programId,
    })
    .rpc();
  return { poolState: r.poolState, tx };
}

export async function showPool(cfg: CliConfig, poolId: PublicKey): Promise<void> {
  const payer = loadPayer(cfg);
  const { program } = buildProgram(cfg, payer);
  const p = await program.account.poolState.fetch(poolId);
  console.log(JSON.stringify(
    {
      address: poolId.toBase58(),
      ammConfig: p.ammConfig.toBase58(),
      poolCreator: p.poolCreator.toBase58(),
      token0Mint: p.token0Mint.toBase58(),
      token1Mint: p.token1Mint.toBase58(),
      token0Vault: p.token0Vault.toBase58(),
      token1Vault: p.token1Vault.toBase58(),
      lpMint: p.lpMint.toBase58(),
      lpSupply: p.lpSupply.toString(),
      openTime: p.openTime.toString(),
      status: p.status,
      mint0Decimals: p.mint0Decimals,
      mint1Decimals: p.mint1Decimals,
      observationKey: p.observationKey.toBase58(),
      pythPriceFeedId: p.pythPriceFeedId,
      minSpreadBps: p.minSpreadBps,
      inventorySkewEnabled: p.inventorySkewEnabled,
      inventorySkewDeadzoneBps: p.inventorySkewDeadzoneBps,
      inventorySkewBpsPerPct: p.inventorySkewBpsPerPct,
      inventorySkewMaxBps: p.inventorySkewMaxBps,
      oracleKeeper: p.oracleKeeper.toBase58(),
      effectiveBidMantissa: p.effectiveBidMantissa.toString(),
      effectiveAskMantissa: p.effectiveAskMantissa.toString(),
      priceExponent: p.priceExponent,
      dynamicFeeRate: p.dynamicFeeRate.toString(),
      lastOracleUpdateSlot: p.lastOracleUpdateSlot.toString(),
      lastOracleUpdateUnix: p.lastOracleUpdateUnix.toString(),
    },
    null,
    2,
  ));
}
