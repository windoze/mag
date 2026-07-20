import type { ConfigDto } from "@mag/protocol";
import { TomlError, parse, stringify } from "smol-toml";

/**
 * Serializes a ConfigDto to TOML text for the text-mode ConfigEditor.
 *
 * TOML has no null/undefined, so both are deep-stripped first. Empty arrays
 * and empty tables are kept: the server distinguishes an absent key from an
 * explicit empty value (e.g. `tools = []` exposes no tools while a missing
 * `tools` key leaves the agent unconstrained), so dropping them would
 * silently change semantics on save. Secret references (`{ env = "..." }` /
 * `{ keyring = "..." }` inline tables) pass through unchanged and are never
 * materialized.
 *
 * Known round-trip caveat: free-form tables (`ProviderDto.params`) flow
 * JSON → JS → TOML, so scalar types can shift — TOML datetimes come back as
 * strings and integral floats (`2.0`) are re-emitted as integers. Structured
 * DTO fields are unaffected.
 */
export function configDtoToToml(dto: ConfigDto): string {
  const stripped = stripUnsupported(dto) as Record<string, unknown>;
  if (Object.keys(stripped).length === 0) {
    return "";
  }
  return stringify(stripped);
}

/**
 * Parses ConfigEditor TOML text back into a ConfigDto. Empty or
 * whitespace-only text maps to an empty DTO; invalid TOML throws a clear
 * Error whose message is safe to show in the UI. Keys outside the ConfigDto
 * shape are rejected instead of silently dropped — `update_config` replaces
 * the whole persisted config, so an unrecognized (e.g. typo'd) section would
 * otherwise vanish from disk on save.
 */
export function tomlToConfigDto(text: string): ConfigDto {
  if (text.trim().length === 0) {
    return {};
  }

  let parsed: unknown;
  try {
    parsed = parse(text);
  } catch (error: unknown) {
    if (error instanceof TomlError) {
      throw new Error(`Invalid TOML: ${error.message}`);
    }
    throw error;
  }

  if (!isPlainTable(parsed)) {
    throw new Error("Invalid TOML: the config document must be a table");
  }
  assertKnownKeys(parsed, "", CONFIG_KEY_SCHEMA);
  return parsed as ConfigDto;
}

/** Removes null/undefined entries deep; keeps empty arrays and empty tables. */
function stripUnsupported(value: unknown): unknown {
  if (Array.isArray(value)) {
    return value
      .filter((item) => item !== undefined && item !== null)
      .map((item) => stripUnsupported(item));
  }
  if (typeof value === "object" && value !== null) {
    return Object.fromEntries(
      Object.entries(value)
        .filter(([, entry]) => entry !== undefined && entry !== null)
        .map(([key, entry]) => [key, stripUnsupported(entry)])
    );
  }
  return value;
}

function isPlainTable(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/**
 * Key whitelist mirroring the ConfigDto wire shape (generated protocol
 * types). Free-form tables (`params`, `env` values) are not validated.
 * When the ConfigDto wire shape changes, update this map alongside the
 * regenerated protocol types.
 */
const BUDGET_KEYS: Readonly<Record<string, unknown>> = {
  max_steps: null,
  max_tokens: null,
  max_cost_micros: null,
  max_wall_time_secs: null
};

const CONFIG_KEY_SCHEMA: Readonly<Record<string, unknown>> = {
  providers: {
    "*": {
      wire: null,
      base_url: null,
      api_key: { env: null, keyring: null },
      params: null // free-form table, not validated
    }
  },
  agents: {
    "*": {
      provider: null,
      model: null,
      tools: null,
      system_prompt: null,
      role: null,
      budget: BUDGET_KEYS
    }
  },
  external_agents: {
    "*": {
      kind: null,
      command: null,
      env: null, // free-form values, not validated
      capabilities: null
    }
  },
  tools: { "*": { approval: null, enabled: null } },
  session: { routing: null, persist_path: null, budget: BUDGET_KEYS },
  approval: { default_policy: null, timeout_secs: null }
};

/**
 * Recursively rejects unknown keys. `schema` maps a known key to either
 * `null` (leaf, no further validation) or a nested schema; the `"*"` entry
 * applies to every key of a name-keyed map section.
 */
function assertKnownKeys(
  table: Record<string, unknown>,
  path: string,
  schema: Readonly<Record<string, unknown>>
): void {
  for (const [key, value] of Object.entries(table)) {
    // `null` marks a validated leaf, so fall back to "*" only when the key
    // itself is absent from the schema.
    const nested = key in schema ? schema[key] : schema["*"];
    if (nested === undefined) {
      const fullPath = path.length === 0 ? key : `${path}.${key}`;
      throw new Error(`Invalid TOML: unknown config key \`${fullPath}\``);
    }
    if (nested !== null && isPlainTable(value)) {
      const fullPath = path.length === 0 ? key : `${path}.${key}`;
      assertKnownKeys(value, fullPath, nested as Readonly<Record<string, unknown>>);
    }
  }
}
