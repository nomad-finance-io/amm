import { AnchorProvider, Idl, Program, Wallet } from "@coral-xyz/anchor";
import { Keypair } from "@solana/web3.js";
import { NomadAmm } from "../../target/types/nomad_amm";
import idlJson from "../../target/idl/nomad_amm.json";
import { CliConfig, loadKeypair, makeConnection } from "./config";

export interface ProgramHandle {
  program: Program<NomadAmm>;
  provider: AnchorProvider;
  payer: Keypair;
}

/** Build a Program handle whose provider is signed by `payer`. */
export function buildProgram(cfg: CliConfig, payer: Keypair): ProgramHandle {
  const connection = makeConnection(cfg);
  const wallet = new Wallet(payer);
  const provider = new AnchorProvider(connection, wallet, { commitment: "confirmed" });
  // anchor reads the program id from idl.address; override when the user
  // supplied --program-id (so devnet/mainnet builds work without rebuilding).
  const idl = { ...(idlJson as Idl), address: cfg.programId.toBase58() };
  const program = new Program(idl as NomadAmm, provider);
  return { program, provider, payer };
}

export function loadAdmin(cfg: CliConfig): Keypair {
  return loadKeypair(cfg.adminPath);
}

export function loadPayer(cfg: CliConfig): Keypair {
  return loadKeypair(cfg.payerPath);
}
