import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import * as ini from "ini";
import * as dotenv from "dotenv";
import { Connection, Keypair, PublicKey } from "@solana/web3.js";
import idl from "../../target/idl/nomad_amm.json";

const HOME = os.homedir();
const DEFAULT_KEYPAIR = path.join(HOME, ".config", "solana", "id.json");
const SOLANA_CLI_CONFIG = path.join(HOME, ".config", "solana", "cli", "config.yml");
// repo-root .env, regardless of where the cli was invoked from
const REPO_ENV = path.resolve(__dirname, "..", "..", ".env");
dotenv.config({ path: REPO_ENV });

interface SolanaCliConfig {
  keypairPath?: string;
  jsonRpcUrl?: string;
  websocketUrl?: string;
}

/** Minimal scalar-only YAML reader for the Solana CLI config (`key: value`). */
function readSolanaCliConfig(): SolanaCliConfig {
  if (!fs.existsSync(SOLANA_CLI_CONFIG)) return {};
  const out: SolanaCliConfig = {};
  for (const line of fs.readFileSync(SOLANA_CLI_CONFIG, "utf8").split("\n")) {
    const m = line.match(/^\s*([a-z_]+)\s*:\s*(.+?)\s*$/);
    if (!m) continue;
    const value = m[2].replace(/^['"]|['"]$/g, "");
    if (m[1] === "keypair_path") out.keypairPath = value;
    else if (m[1] === "json_rpc_url") out.jsonRpcUrl = value;
    else if (m[1] === "websocket_url") out.websocketUrl = value;
  }
  return out;
}

function expandHome(p: string): string {
  return p.startsWith("~/") ? path.join(HOME, p.slice(2)) : p;
}

export interface CliFlags {
  config?: string;
  rpcUrl?: string;
  keypair?: string;
  adminKeypair?: string;
  programId?: string;
}

export interface CliConfig {
  httpUrl: string;
  wsUrl: string | undefined;
  payerPath: string;
  adminPath: string;
  programId: PublicKey;
}

/**
 * Resolve config by layering, last write wins:
 *   1. defaults (mainnet RPC, `~/.config/solana/id.json`, IDL's program id)
 *   2. `~/.config/solana/cli/config.yml` (the standard Solana CLI config)
 *   3. `.env` HELIUS_API_KEY → constructs the Helius mainnet URL
 *   4. `--config <client_config.ini>` if passed
 *   5. CLI flags (`--rpc-url`, `--keypair`, `--admin-keypair`, `--program-id`)
 */
export function resolveConfig(flags: CliFlags): CliConfig {
  const solanaCli = readSolanaCliConfig();
  let httpUrl = process.env.SOLANA_RPC_URL?.trim();
  let wsUrl = solanaCli.websocketUrl;
  let payerPath = expandHome(solanaCli.keypairPath ?? DEFAULT_KEYPAIR);
  let adminPath = payerPath;
  let programId = new PublicKey((idl as { address: string }).address);

  if (flags.config) {
    const absPath = path.resolve(flags.config);
    const raw = fs.readFileSync(absPath, "utf-8");
    const parsed = ini.parse(raw);
    const g = (parsed.Global ?? {}) as Record<string, string>;
    const baseDir = path.dirname(absPath);
    const resolveKp = (p: string) => {
      const expanded = expandHome(p);
      return path.isAbsolute(expanded) ? expanded : path.join(baseDir, expanded);
    };
    if (g.http_url) httpUrl = g.http_url;
    if (g.ws_url) wsUrl = g.ws_url;
    if (g.payer_path) payerPath = resolveKp(g.payer_path);
    if (g.admin_path) adminPath = resolveKp(g.admin_path);
    if (g.raydium_cp_program) programId = new PublicKey(g.raydium_cp_program);
  }

  if (flags.rpcUrl) httpUrl = flags.rpcUrl;
  if (flags.keypair) payerPath = expandHome(flags.keypair);
  if (flags.adminKeypair) adminPath = expandHome(flags.adminKeypair);
  if (flags.programId) programId = new PublicKey(flags.programId);

  return { httpUrl, wsUrl, payerPath, adminPath, programId };
}

export function loadKeypair(filePath: string): Keypair {
  const raw = fs.readFileSync(filePath, "utf-8");
  return Keypair.fromSecretKey(Uint8Array.from(JSON.parse(raw) as number[]));
}

export function makeConnection(cfg: CliConfig): Connection {
  return new Connection(cfg.httpUrl, {
    wsEndpoint: cfg.wsUrl,
    commitment: "confirmed",
  });
}
