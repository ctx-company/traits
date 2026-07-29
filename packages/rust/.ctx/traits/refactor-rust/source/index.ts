import { rustReviewerRole, rustWorkerRole } from "@ctx-traits/rust";
import { dependency, port, procedure, prompt, ref, sequence, slot, trait } from "@ctx-traits/cdk";

const worker = rustWorkerRole("worker", "Implements the requested Rust refactor and holds it to the repo's own gates.");
const reviewer = rustReviewerRole(
    "reviewer",
    "Reviews the implemented Rust refactor once against the repo's engineering-standards and gate-conventions.",
);

const target = port.input.text({
    id: "target",
    description: "Crate, module, or file path to refactor.",
});

// --- Dependency composition: the rust pack's own standards, consumed by reference. ---
const engineeringStandards = ref.resource({ id: "engineering-standards", dependency: "rust" });
const gateConventions = ref.resource({ id: "gate-conventions", dependency: "rust" });

const workSummary = slot.text({
    id: "work-summary",
    description: "Worker's report of the refactored state.",
    hint: "What changed (files), and the exact gate commands (fmt --check, check, clippy) run and their result.",
});
const reviewNotes = slot.text({
    id: "review-notes",
    description: "Reviewer's judgment of the implemented refactor against the gate-conventions and engineering-standards.",
    hint: "Whether the gates actually pass, and any blocking defect found by inspecting the working tree directly.",
});

const workReport = port.output.text({
    id: "work-report",
    description: "Final report of the refactored state.",
    value: workSummary,
});
const reviewReport = port.output.text({
    id: "review-report",
    description: "Final reviewer judgment of the refactored state.",
    value: reviewNotes,
});

export default trait("refactor-rust", {
    version: "0.1.0",
    name: "Refactor (Rust)",
    summary:
        "Minimal Rust refactor procedure: implement the requested change and hold it to this repo's Rust engineering-standards and gate-conventions, then review once against the same doctrine — no refinement loop, no commit.",
    metadata: {
        family: "refactor",
        variant: "rust",
        tag: ["first-party", "refactoring", "rust"],
    },
    dependency: dependency({ alias: "rust", id: "rust", version: "0.1.0", source: { path: "../rust" } }),
    procedure: procedure({
        description:
            "Implement a Rust refactor for the target, holding it to the rust pack's engineering-standards and gate-conventions, then review it once against the same doctrine.",
        input: target,
        output: [workReport, reviewReport],
        sequence: [
            sequence.prompt("implement", {
                title: "Implement the refactor (worker)",
                agent: worker,
                text: prompt.text`
                    Implement the requested refactor for ${target}.
                    Hold the change to the engineering-standards ${engineeringStandards} and run the gates named in ${gateConventions} (fmt --check, check, clippy -D warnings) before reporting.
                    Return a work summary: what changed (files), the exact gate commands you ran, and their result.`,
                output: workSummary,
                input: [target, engineeringStandards, gateConventions],
            }),
            sequence.prompt("review", {
                title: "Review the refactor (reviewer)",
                agent: reviewer,
                text: prompt.text`
                    Review the refactored state of ${target} against the work summary ${workSummary}.
                    Consult the engineering-standards ${engineeringStandards} and gate-conventions ${gateConventions}, and inspect the actual working tree and gate output with your tools — never review the summary alone.
                    Return your judgment: whether the gates genuinely pass and the standards are held, naming any blocking defect found.`,
                output: reviewNotes,
                input: [target, workSummary, engineeringStandards, gateConventions],
            }),
        ],
    }),
});
