import type { ConfigDto } from "@mag/protocol";
import { TomlError, parse, stringify } from "smol-toml";

/**
 * Serializes a ConfigDto to TOML text for the text-mode ConfigEditor.
 *
 * TOML has no null/undefined, so both are deep-stripped first; objects and
 * arrays left empty by stripping are dropped as well so the editor text stays
 * minimal. Secret references (`{ env = "..." }` / `{ keyring = "..." }`
 * inline tables) pass through unchanged and are never materialized.
 */
export function configDtoToToml(dto: ConfigDto): string {
  const stripped = stripUnsupported(dto);
  if (stripped === DROP) {
    return "";
  }
  return stringify(stripped as Record<string, unknown>);
}

/**
 * Parses ConfigEditor TOML text back into a ConfigDto. Empty or
 * whitespace-only text maps to an empty DTO; invalid TOML throws a clear
 * Error whose message is safe to show in the UI.
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

  if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
    throw new Error("Invalid TOML: the config document must be a table");
  }
  return parsed as ConfigDto;
}

/** Sentinel returned by `stripUnsupported` for values TOML cannot represent. */
const DROP = Symbol("drop");

type StripResult = unknown | typeof DROP;

function stripUnsupported(value: unknown): StripResult {
  if (value === undefined || value === null) {
    return DROP;
  }
  if (Array.isArray(value)) {
    const items = value
      .map((item) => stripUnsupported(item))
      .filter((item): item is Exclude<StripResult, typeof DROP> => item !== DROP);
    return items.length === 0 ? DROP : items;
  }
  if (typeof value === "object") {
    const entries = Object.entries(value)
      .map(([key, entry]) => [key, stripUnsupported(entry)] as const)
      .filter(
        (entry): entry is readonly [string, Exclude<StripResult, typeof DROP>] => entry[1] !== DROP
      );
    if (entries.length === 0) {
      return DROP;
    }
    return Object.fromEntries(entries);
  }
  return value;
}
