/**
 * `signal.{Abort, Continue, Skip, Park}` (0209): the four raise-only verb
 * constants an authoring site passes to `loop.maxIterations({ onExhausted })`,
 * `flow.when(title, cond, signal.Abort)`, or `effect.onFailure(...)` to
 * decide how the enclosing structure reacts, instead of a bare policy
 * string. A dependency-free leaf module — `procedure.ts` (mints `signal.*`),
 * `condition.ts` (rejects a verb passed to `condition.signal`), and
 * `functional/registrars.ts` (maps a verb to its site's canonical policy
 * string) all need this without importing each other.
 *
 * `signal.Abort` means "fail the enclosing structure and propagate" — it is
 * not `flow.error` (which ends the whole run). Site-legality and
 * at-most-one-verb-per-event rules live where each verb is consumed, not
 * here.
 */

const SIGNAL_VERB_BRAND: unique symbol = Symbol("signalVerb");

export type SignalVerbName = "abort" | "continue" | "skip" | "park";

/** A branded `signal.*` verb constant. Never itself a canonical value — every consumer maps it to today's policy string before it reaches a declaration. */
export interface SignalVerb<Name extends SignalVerbName = SignalVerbName> {
  readonly [SIGNAL_VERB_BRAND]: Name;
}

const VERB_LABELS: Readonly<Record<SignalVerbName, string>> = {
  abort: "signal.Abort",
  continue: "signal.Continue",
  skip: "signal.Skip",
  park: "signal.Park",
};

/** The display label used in build-error messages, e.g. `"signal.Abort"`. */
export function signalVerbLabel(name: SignalVerbName): string {
  return VERB_LABELS[name];
}

function makeVerb<Name extends SignalVerbName>(name: Name): SignalVerb<Name> {
  return Object.freeze({ [SIGNAL_VERB_BRAND]: name }) as SignalVerb<Name>;
}

/** `signal.Abort`: fail the enclosing structure (loop/parallel branch) and propagate. */
export const SIGNAL_ABORT: SignalVerb<"abort"> = makeVerb("abort");
/** `signal.Continue`: proceed past the enclosing structure's exhaustion/failure as a normal outcome. */
export const SIGNAL_CONTINUE: SignalVerb<"continue"> = makeVerb("continue");
/** `signal.Skip`: drop the failed unit (a parallel branch) and proceed without it. */
export const SIGNAL_SKIP: SignalVerb<"skip"> = makeVerb("skip");
/** `signal.Park`: set the failed unit aside for later, honest triage instead of silently dropping it. */
export const SIGNAL_PARK: SignalVerb<"park"> = makeVerb("park");

/** Narrows an arbitrary authoring-site value to its verb name, or `undefined` when it isn't a `signal.*` verb constant. */
export function signalVerbOf(value: unknown): SignalVerbName | undefined {
  if (typeof value !== "object" || value === null) return undefined;
  return (value as Partial<SignalVerb>)[SIGNAL_VERB_BRAND];
}
