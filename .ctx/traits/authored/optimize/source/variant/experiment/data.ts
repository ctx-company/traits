import { port, schema } from "@ctx-traits/cdk";

export const objective = port.input.text({
  id: "objective",
  description: "The bounded optimization objective for this experiment run.",
});

export const metricField = port.input.text({
  id: "metric-field",
  description: "Human-readable meaning and unit normalized into the aggregate result's metric.",
});

export const experimentCommand = port.input.of(schema.list(schema.text()), {
  id: "experiment-command",
  description:
    'No-shell argv executed for one baseline or candidate run; must emit JSON matching { status: "ok" | "error", metric: number, "delta-lines"?: number }. The optional delta-lines feeds the max-delta-lines cap; omit it if unmeasured.',
});
