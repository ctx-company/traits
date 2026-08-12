import { recordTraitMint } from "./functional/context.js";
import type { SettingHandle } from "./handles.js";
import type { CdkObject } from "./meta.js";
import { withDeclaration } from "./meta.js";
import { compact, validateSlug } from "./normalize.js";

interface SettingFields<Value> {
  readonly id: string;
  readonly description: string;
  readonly default: Value;
}

interface NumberSettingFields extends SettingFields<number> {
  readonly min?: number;
  readonly max?: number;
}

/**
 * Declares a typed operator knob: a `[[setting]]` resolved at ACTIVATION
 * from config layers (canonical default < global config < repo config),
 * never at build time. Accepted anywhere a slot/port input is — prompt
 * interpolation, `input.command` argv tokens, condition comparisons,
 * include lists, structural fields (e.g. `loop.maxIterations`) — the same
 * way a `SlotHandle`/`PortHandle` is.
 *
 * @example
 * ```ts
 * const rounds = setting.number({
 *   id: "review-rounds",
 *   description: "Number of reviewer loop iterations.",
 *   default: 3,
 *   min: 1,
 *   max: 10,
 * });
 * loop.maxIterations(rounds);
 * ```
 */
export interface SettingFunction {
  number(fields: NumberSettingFields): SettingHandle<number>;
  text(fields: SettingFields<string>): SettingHandle<string>;
  boolean(fields: SettingFields<boolean>): SettingHandle<boolean>;
}

function settingOf<Value>(
  schema: "number" | "text" | "boolean",
  fields: SettingFields<Value> & { readonly min?: number; readonly max?: number },
): SettingHandle<Value> {
  validateSlug(fields.id, "setting.id");
  const declaration = compact({
    id: fields.id,
    schema,
    description: fields.description,
    default: fields.default,
    min: fields.min,
    max: fields.max,
  });
  const handle = withDeclaration<CdkObject, "setting", Value>("setting", `setting:${fields.id}`, declaration, {}, {});
  recordTraitMint("setting", fields.id, `setting:${fields.id}`, declaration);
  return handle;
}

export const setting: SettingFunction = {
  number: (fields) => settingOf<number>("number", fields),
  text: (fields) => settingOf<string>("text", fields),
  boolean: (fields) => settingOf<boolean>("boolean", fields),
};
