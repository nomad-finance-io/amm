import { PublicKey, SystemProgram } from "@solana/web3.js";
import { CliConfig } from "../config";
import { buildProgram, loadAdmin } from "../program";
import { findPermission } from "../pdas";

export async function createPermission(
  cfg: CliConfig,
  authority: PublicKey,
): Promise<{ permission: PublicKey; tx: string }> {
  const admin = loadAdmin(cfg);
  const { program } = buildProgram(cfg, admin);
  const permission = findPermission(authority, cfg.programId);
  const tx = await program.methods
    .createPermissionPda()
    .accounts({
      owner: admin.publicKey,
      permissionAuthority: authority,
      permission,
      systemProgram: SystemProgram.programId,
    })
    .rpc();
  return { permission, tx };
}

export async function closePermission(cfg: CliConfig, authority: PublicKey): Promise<string> {
  const admin = loadAdmin(cfg);
  const { program } = buildProgram(cfg, admin);
  const permission = findPermission(authority, cfg.programId);
  return program.methods
    .closePermissionPda()
    .accounts({
      owner: admin.publicKey,
      permissionAuthority: authority,
      permission,
      systemProgram: SystemProgram.programId,
    })
    .rpc();
}
