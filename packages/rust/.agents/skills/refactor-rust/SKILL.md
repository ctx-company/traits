---
name: "Refactor (Rust)"
description: "Minimal Rust refactor procedure: implement the requested change and hold it to this repo's Rust engineering-standards and gate-conventions, then review once against the same doctrine — no refinement loop, no commit."
---

> GENERATED FILE - DO NOT EDIT DIRECTLY
> Source trait: refactor-rust
> Source digest: sha256:1ea14bdb28c5211001f836745c3c366b04d55b0a5c746e81ef0feea0ec467305
> Render profile: agent-skills
> Edit the canonical source trait file instead.

# refactor-rust

<trait id="refactor-rust" version="0.1.0" model-view="sha256:4203bf72799ebd2fb17bdbee044e68c4f139e0af9e74c454eaf4e5b60497c536">
  <summary>
    Minimal Rust refactor procedure: implement the requested change and hold it to this repo's Rust engineering-standards and gate-conventions, then review once against the same doctrine — no refinement loop, no commit.
  </summary>
  <resource id="rust/engineering-standards" digest="none" render="reference">
    resource:rust/engineering-standards available by reference; no hint declared
  </resource>
  <resource id="rust/gate-conventions" digest="none" render="reference">
    resource:rust/gate-conventions available by reference; no hint declared
  </resource>
  <agent id="worker">
    Static render advisory: agent roles describe intended frame routing for the ctx controlled runtime; static Markdown cannot enforce multi-agent routing, harness selection, or caller identity.
    Description: Implements the requested Rust refactor and holds it to the repo's own gates. Holds every change to this repo's Rust engineering-standards and gate-conventions.
    Summary: Implementation role.
    Assigned sequence items: procedure.sequence[0] implement (Implement the refactor (worker))
  </agent>
  <agent id="reviewer">
    Static render advisory: agent roles describe intended frame routing for the ctx controlled runtime; static Markdown cannot enforce multi-agent routing, harness selection, or caller identity.
    Description: Reviews the implemented Rust refactor once against the repo's engineering-standards and gate-conventions. Judges the work against this repo's Rust engineering-standards and gate-conventions.
    Summary: Rust review role.
    Assigned sequence items: procedure.sequence[1] review (Review the refactor (reviewer))
  </agent>
  <prompt id="implement">
    Input: port:target, resource:rust/engineering-standards, resource:rust/gate-conventions
    Output: slot:work-summary
    Body: 
                        Implement the requested refactor for {port:target}.
                        Hold the change to the engineering-standards {resource:rust/engineering-standards} and run the gates named in {resource:rust/gate-conventions} (fmt --check, check, clippy -D warnings) before reporting.
                        Return a work summary: what changed (files), the exact gate commands you ran, and their result.
  </prompt>
  <prompt id="review">
    Input: port:target, slot:work-summary, resource:rust/engineering-standards, resource:rust/gate-conventions
    Output: slot:review-notes
    Body: 
                        Review the refactored state of {port:target} against the work summary {slot:work-summary}.
                        Consult the engineering-standards {resource:rust/engineering-standards} and gate-conventions {resource:rust/gate-conventions}, and inspect the actual working tree and gate output with your tools — never review the summary alone.
                        Return your judgment: whether the gates genuinely pass and the standards are held, naming any blocking defect found.
  </prompt>
  <procedure>
    Description: Implement a Rust refactor for the target, holding it to the rust pack's engineering-standards and gate-conventions, then review it once against the same doctrine.
    Input: port:target
    Output: none
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
    Output ports: none
  </procedure>
  <port id="target" direction="input">
    Optionality: required
    Schema: schema:text
    Backing slot: none
    Description: Crate, module, or file path to refactor.
  </port>
</trait>
