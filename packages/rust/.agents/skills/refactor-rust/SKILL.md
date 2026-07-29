---
name: "Refactor (Rust)"
description: "Minimal Rust refactor procedure: implement the requested change and hold it to this repo's Rust engineering-standards and gate-conventions, then review once against the same doctrine — no refinement loop, no commit."
---

> GENERATED FILE - DO NOT EDIT DIRECTLY
> Source trait: refactor-rust
> Source digest: sha256:bee5a92a1e15dfa2791837de14b278d0f46e39904aae11607476c92860c84557
> Render profile: agent-skills
> Edit the canonical source trait file instead.

# refactor-rust

## Provenance

Trait ID: refactor-rust
Trait version: 0.1.0
Render profile: agent-skills
Source digest: sha256:bee5a92a1e15dfa2791837de14b278d0f46e39904aae11607476c92860c84557

## Identity

ID: refactor-rust
Name: Refactor (Rust)
Summary: Minimal Rust refactor procedure: implement the requested change and hold it to this repo's Rust engineering-standards and gate-conventions, then review once against the same doctrine — no refinement loop, no commit.

## Agent Roles Advisory

Static render advisory: agent roles describe intended frame routing for the ctx controlled runtime; static Markdown cannot enforce multi-agent routing, harness selection, or caller identity.
- agent:worker
  Description: Implements the requested Rust refactor and holds it to the repo's own gates. Holds every change to this repo's Rust engineering-standards and gate-conventions.
  Summary: Implementation role.
  Assigned sequence items: procedure.sequence[0] implement (Implement the refactor (worker))
- agent:reviewer
  Description: Reviews the implemented Rust refactor once against the repo's engineering-standards and gate-conventions. Judges the work against this repo's Rust engineering-standards and gate-conventions.
  Summary: Rust review role.
  Assigned sequence items: procedure.sequence[1] review (Review the refactor (reviewer))

## Prompts

- prompt:implement
  Input: port:target, resource:rust/engineering-standards, resource:rust/gate-conventions
  Output: slot:work-summary
  Body: 
                    Implement the requested refactor for {port:target}.
                    Hold the change to the engineering-standards {resource:rust/engineering-standards} and run the gates named in {resource:rust/gate-conventions} (fmt --check, check, clippy -D warnings) before reporting.
                    Return a work summary: what changed (files), the exact gate commands you ran, and their result.
- prompt:review
  Input: port:target, slot:work-summary, resource:rust/engineering-standards, resource:rust/gate-conventions
  Output: slot:review-notes
  Body: 
                    Review the refactored state of {port:target} against the work summary {slot:work-summary}.
                    Consult the engineering-standards {resource:rust/engineering-standards} and gate-conventions {resource:rust/gate-conventions}, and inspect the actual working tree and gate output with your tools — never review the summary alone.
                    Return your judgment: whether the gates genuinely pass and the standards are held, naming any blocking defect found.

## Procedure

Description: Implement a Rust refactor for the target, holding it to the rust pack's engineering-standards and gate-conventions, then review it once against the same doctrine.
Input: port:target
Output: port:work-report, port:review-report
Static host note: this render describes the procedure contract and runtime-only sequence-control declarations but cannot enforce sequence state, slot validation, command execution, loop exits, for-each iteration, or runtime completion outside the ctx controlled runtime.
1. [implement] Implement the refactor (worker)
  Agent: agent:worker
  Prompt: prompt:implement
  Input: port:target, resource:rust/engineering-standards, resource:rust/gate-conventions
  Output: slot:work-summary
  Format: none
  Emits: none
2. [review] Review the refactor (reviewer)
  Agent: agent:reviewer
  Prompt: prompt:review
  Input: port:target, slot:work-summary, resource:rust/engineering-standards, resource:rust/gate-conventions
  Output: slot:review-notes
  Format: none
  Emits: none
Output ports: port:work-report, port:review-report

## Ports

- port:target (input, required)
  Schema: schema:text
  Value: none
  Description: Crate, module, or file path to refactor.
- port:work-report (output, required)
  Schema: schema:text
  Value: slot:work-summary
  Description: Final report of the refactored state.
- port:review-report (output, required)
  Schema: schema:text
  Value: slot:review-notes
  Description: Final reviewer judgment of the refactored state.

## Final Output Contracts

- port:work-report - Output port work-report
  Description: Final report of the refactored state.
  Schema: schema:text
  Format: none
  Required: true
  Backing slot: slot:work-summary
- port:review-report - Output port review-report
  Description: Final reviewer judgment of the refactored state.
  Schema: schema:text
  Format: none
  Required: true
  Backing slot: slot:review-notes

## Resources

- resource:rust/engineering-standards
  Path: available (openable at runtime; path resolved per session and omitted from the static compile for cross-checkout reproducibility)
  Trigger: OnActivation
  Render: Reference
  Template inputs: none
  Nested resources: none
  Dependency-pending inputs: none
  Unresolved inputs: none
  Unused inputs: none
  Body digest: none
  Inclusion reason: activation-triggered
  Evidence: no digest evidence
  Static note: resource bodies are supplied separately by IO/runtime evidence; static hosts may only see this contract.
- resource:rust/gate-conventions
  Path: available (openable at runtime; path resolved per session and omitted from the static compile for cross-checkout reproducibility)
  Trigger: OnActivation
  Render: Reference
  Template inputs: none
  Nested resources: none
  Dependency-pending inputs: none
  Unresolved inputs: none
  Unused inputs: none
  Body digest: none
  Inclusion reason: activation-triggered
  Evidence: no digest evidence
  Static note: resource bodies are supplied separately by IO/runtime evidence; static hosts may only see this contract.

