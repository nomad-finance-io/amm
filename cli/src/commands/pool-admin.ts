import { PublicKey } from "@solana/web3.js";
import { CliConfig } from "../config";
import { buildProgram, loadAdmin } from "../program";

export async function setPoolStatus(
  cfg: CliConfig,
  poolId: PublicKey,
  status: number,
): Promise<string> {
  if (status < 0 || status > 255) throw new Error("status must be 0..255");
  const admin = loadAdmin(cfg);
  const { program } = buildProgram(cfg, admin);
  return program.methods
    .updatePoolStatus(status)
    .accounts({ authority: admin.publicKey, poolState: poolId })
    .rpc();
}

export async function setOracleKeeper(
  cfg: CliConfig,
  poolId: PublicKey,
  newKeeper: PublicKey,
): Promise<string> {
  const admin = loadAdmin(cfg);
  const { program } = buildProgram(cfg, admin);
  return program.methods
    .setOracleKeeper(newKeeper)
    .accounts({ authority: admin.publicKey, poolState: poolId })
    .rpc();
}
