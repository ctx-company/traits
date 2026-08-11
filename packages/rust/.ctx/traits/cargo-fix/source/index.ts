import { cargoFixLoop, rustWorkerRole } from "@ctx-traits/rust";
import { port, procedure, trait } from "@ctx-traits/cdk";

const worker = rustWorkerRole("worker", "Fixes cargo check/clippy diagnostics until a fresh capture is clean.");

const scope = port.input.text({
    id: "scope",
    description:
        "Optional -p <package> scope passed to both cargo check and cargo clippy; empty scopes to the whole workspace.",
    default: { value: "" },
});

export default trait("cargo-fix", {
    version: "0.1.0",
    name: "Cargo Fix",
    description:
        "The compiler is the reviewer: capture and reduce a real cargo check + clippy run into a deduplicated, deterministically ordered diagnostic list, then drive a worker through bounded fix rounds — each followed by a fresh recapture — until the list is clean. A round that cannot clear it blocks rather than committing.",
    metadata: {
        family: "cargo-fix",
        tag: ["first-party", "rust", "gate-driven"],
    },
    procedure: procedure({
        description:
            "Capture cargo check/clippy diagnostics, drive the worker through bounded fix rounds against the reduced list, and block if a fresh capture never comes back clean.",
        input: scope,
        sequence: [...cargoFixLoop({ worker, scope: { id: "scope", port: scope }, rounds: 4 })],
    }),
});
