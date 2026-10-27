// Set with `wrangler secret put`, so `wrangler types` cannot see them.
interface Env {
  DISCORD_CLIENT_SECRET: string;
  /** The key of the fingerprint a deleted account leaves behind (see departed.ts). */
  DEPARTED_PEPPER?: string;
}
