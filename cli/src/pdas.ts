import { PublicKey } from "@solana/web3.js";

const enc = (s: string) => Buffer.from(s, "utf8");

export const AMM_CONFIG_SEED = enc("amm_config");
export const POOL_SEED = enc("pool");
export const POOL_VAULT_SEED = enc("pool_vault");
export const POOL_AUTH_SEED = enc("vault_and_lp_mint_auth_seed");
export const POOL_LP_MINT_SEED = enc("pool_lp_mint");
export const ORACLE_SEED = enc("observation");
export const PERMISSION_SEED = enc("permission");

export function u16beBytes(num: number): Uint8Array {
  const buf = Buffer.alloc(2);
  buf.writeUInt16BE(num, 0);
  return buf;
}

export function findAmmConfig(index: number, programId: PublicKey): PublicKey {
  return PublicKey.findProgramAddressSync([AMM_CONFIG_SEED, u16beBytes(index)], programId)[0];
}

export function findAuthority(programId: PublicKey): PublicKey {
  return PublicKey.findProgramAddressSync([POOL_AUTH_SEED], programId)[0];
}

export function findPool(
  ammConfig: PublicKey,
  token0: PublicKey,
  token1: PublicKey,
  programId: PublicKey,
): PublicKey {
  return PublicKey.findProgramAddressSync(
    [POOL_SEED, ammConfig.toBuffer(), token0.toBuffer(), token1.toBuffer()],
    programId,
  )[0];
}

export function findPoolVault(pool: PublicKey, mint: PublicKey, programId: PublicKey): PublicKey {
  return PublicKey.findProgramAddressSync(
    [POOL_VAULT_SEED, pool.toBuffer(), mint.toBuffer()],
    programId,
  )[0];
}

export function findLpMint(pool: PublicKey, programId: PublicKey): PublicKey {
  return PublicKey.findProgramAddressSync([POOL_LP_MINT_SEED, pool.toBuffer()], programId)[0];
}

export function findObservation(pool: PublicKey, programId: PublicKey): PublicKey {
  return PublicKey.findProgramAddressSync([ORACLE_SEED, pool.toBuffer()], programId)[0];
}

export function findPermission(authority: PublicKey, programId: PublicKey): PublicKey {
  return PublicKey.findProgramAddressSync(
    [PERMISSION_SEED, authority.toBuffer()],
    programId,
  )[0];
}
