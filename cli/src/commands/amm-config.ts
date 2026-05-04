import { BN } from "@coral-xyz/anchor";
import { AccountMeta, PublicKey, SystemProgram } from "@solana/web3.js";
import { CliConfig } from "../config";
import { buildProgram, loadAdmin } from "../program";
import { findAmmConfig } from "../pdas";

export async function createAmmConfig(
  cfg: CliConfig,
  args: {
    index: number;
    tradeFeeRate: bigint;
    protocolFeeRate: bigint;
    fundFeeRate: bigint;
    createPoolFee: bigint;
    creatorFeeRate: bigint;
  },
): Promise<{ ammConfig: PublicKey; tx: string }> {
  const admin = loadAdmin(cfg);
  const { program } = buildProgram(cfg, admin);
  const ammConfig = findAmmConfig(args.index, cfg.programId);

  const tx = await program.methods
    .createAmmConfig(
      args.index,
      new BN(args.tradeFeeRate.toString()),
      new BN(args.protocolFeeRate.toString()),
      new BN(args.fundFeeRate.toString()),
      new BN(args.createPoolFee.toString()),
      new BN(args.creatorFeeRate.toString()),
    )
    .accounts({
      owner: admin.publicKey,
      ammConfig,
      systemProgram: SystemProgram.programId,
    })
    .rpc();
  return { ammConfig, tx };
}

/**
 * AmmConfig param ids (mirrors `update_amm_config` in `programs/cp-swap/src/instructions/admin/update_config.rs`):
 *   0 trade_fee_rate (u64)
 *   1 protocol_fee_rate (u64)
 *   2 fund_fee_rate (u64)
 *   3 new protocol owner (pubkey via remaining accounts; value ignored)
 *   4 new fund owner    (pubkey via remaining accounts; value ignored)
 *   5 create_pool_fee (u64)
 *   6 disable_create_pool (0 or 1)
 *   7 creator_fee_rate (u64)
 */
export async function updateAmmConfig(
  cfg: CliConfig,
  args: { index: number; param: number; value: bigint; newKey?: PublicKey },
): Promise<string> {
  const admin = loadAdmin(cfg);
  const { program } = buildProgram(cfg, admin);
  const ammConfig = findAmmConfig(args.index, cfg.programId);

  const remaining: AccountMeta[] = [];
  if (args.param === 3 || args.param === 4) {
    if (!args.newKey) throw new Error(`param ${args.param} requires --new-key`);
    remaining.push({ pubkey: args.newKey, isSigner: false, isWritable: false });
  }

  return program.methods
    .updateAmmConfig(args.param, new BN(args.value.toString()))
    .accounts({ owner: admin.publicKey, ammConfig })
    .remainingAccounts(remaining)
    .rpc();
}

export async function showAmmConfig(cfg: CliConfig, index: number): Promise<void> {
  const admin = loadAdmin(cfg);
  const { program } = buildProgram(cfg, admin);
  const ammConfig = findAmmConfig(index, cfg.programId);
  const acc = await program.account.ammConfig.fetch(ammConfig);
  console.log(JSON.stringify(
    {
      address: ammConfig.toBase58(),
      index: acc.index,
      bump: acc.bump,
      disableCreatePool: acc.disableCreatePool,
      tradeFeeRate: acc.tradeFeeRate.toString(),
      protocolFeeRate: acc.protocolFeeRate.toString(),
      fundFeeRate: acc.fundFeeRate.toString(),
      createPoolFee: acc.createPoolFee.toString(),
      creatorFeeRate: acc.creatorFeeRate.toString(),
      protocolOwner: acc.protocolOwner.toBase58(),
      fundOwner: acc.fundOwner.toBase58(),
    },
    null,
    2,
  ));
}
