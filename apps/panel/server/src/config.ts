import { createHash, randomBytes } from 'crypto';

/**
 * Central runtime configuration.
 *
 * All values are read from the environment (Bun automatically loads `.env`
 * files from the working directory). Generate a production `.env` with
 * `./install.sh` or by hand — see `.env.example` in the repository root.
 */

function readInt(name: string, fallback: number): number {
  const raw = process.env[name];
  if (!raw) return fallback;
  const parsed = Number.parseInt(raw, 10);
  if (Number.isNaN(parsed) || parsed <= 0) {
    throw new Error(`Invalid ${name}: expected a positive integer, got "${raw}"`);
  }
  return parsed;
}

function readBool(name: string, fallback: boolean): boolean {
  const raw = process.env[name];
  if (raw === undefined || raw === '') return fallback;
  return ['1', 'true', 'yes', 'on'].includes(raw.toLowerCase());
}

function fingerprint(value: string): string {
  return createHash('sha256').update(value).digest('hex').slice(0, 8);
}

const isProduction = process.env.NODE_ENV === 'production';

/**
 * Secret used to sign session JWTs. REQUIRED.
 *
 * Refuses to boot with the well-known development default so a misconfigured
 * production deploy can never ship with forgeable tokens.
 */
function resolveJwtSecret(): string {
  const secret = process.env.JWT_SECRET?.trim();

  if (!secret || secret === 'your-secret-key-here' || secret === 'changeme') {
    if (isProduction) {
      throw new Error(
        'JWT_SECRET is not set (or is a known default). ' +
        'Generate one with `openssl rand -hex 32` and put it in .env, then restart.'
      );
    }
    // Development convenience: stable per-boot ephemeral secret instead of a
    // hardcoded value. Sessions reset when the process restarts.
    const ephemeral = randomBytes(32).toString('hex');
    console.warn(
      '[blink] warning: JWT_SECRET not set — using an ephemeral secret for this session. ' +
      `Set it in .env for persistent sessions (fp: ${fingerprint(ephemeral)}).`
    );
    return ephemeral;
  }

  if (secret.length < 32) {
    if (isProduction) {
      throw new Error('JWT_SECRET must be at least 32 characters. Generate one with `openssl rand -hex 32`.');
    }
    console.warn('[blink] warning: JWT_SECRET is shorter than 32 characters — use `openssl rand -hex 32` in production.');
  }

  return secret;
}

/**
 * Key used to sign cargo download URLs handed to nodes. REQUIRED in
 * production; falls back to the JWT secret in development.
 */
function resolveAppKey(): string {
  const key = process.env.APP_KEY?.trim();
  if (key) {
    if (key.length < 32 && isProduction) {
      throw new Error('APP_KEY must be at least 32 characters. Generate one with `openssl rand -hex 32`.');
    }
    return key;
  }
  if (isProduction) {
    throw new Error('APP_KEY is not set. Generate one with `openssl rand -hex 32` and put it in .env.');
  }
  console.warn('[blink] warning: APP_KEY not set — falling back to JWT_SECRET for cargo URL signing.');
  return resolveJwtSecret();
}

export const JWT_SECRET = resolveJwtSecret();
export const APP_KEY = resolveAppKey();
export const PORT = readInt('PORT', 3000);
export const APP_URL = (process.env.APP_URL?.trim() || `http://localhost:${PORT}`).replace(/\/+$/, '');
export const FIRST_USER_HAS_ADMIN = readBool('FIRST_USER_HAS_ADMIN', true);
export const IS_PRODUCTION = isProduction;
