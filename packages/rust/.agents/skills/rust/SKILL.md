---
name: "Rust"
description: "Shared Rust doctrine: worker/reviewer roles specialized for Rust changes, plus the engineering-standards and gate-conventions resources this repo's Rust core is held to, for dependents that declare it instead of pasting the roles or doctrine into their own trait source."
---

> GENERATED FILE - DO NOT EDIT DIRECTLY
> Source trait: rust
> Source digest: sha256:180d6694e703bb8b1612513b08087757328edae3e19a2c01c21db1a289c3f46b
> Render profile: agent-skills
> Edit the canonical source trait file instead.

# rust

## Provenance

Trait ID: rust
Trait version: 0.1.0
Render profile: agent-skills
Source digest: sha256:180d6694e703bb8b1612513b08087757328edae3e19a2c01c21db1a289c3f46b

## Identity

ID: rust
Name: Rust
Summary: Shared Rust doctrine: worker/reviewer roles specialized for Rust changes, plus the engineering-standards and gate-conventions resources this repo's Rust core is held to, for dependents that declare it instead of pasting the roles or doctrine into their own trait source.

## Agent Roles Advisory

Static render advisory: agent roles describe intended frame routing for the ctx controlled runtime; static Markdown cannot enforce multi-agent routing, harness selection, or caller identity.
- agent:worker
  Description: Implements a draft or agreed Rust design and applies reviewer fixes. Holds every change to this repo's Rust engineering-standards and gate-conventions.
  Summary: Implementation role.
  Assigned sequence items: none declared
- agent:reviewer
  Description: Drafts and/or reviews Rust work in a bounded refinement loop. Judges the work against this repo's Rust engineering-standards and gate-conventions.
  Summary: Rust review role.
  Assigned sequence items: none declared

## Resources

- resource:engineering-standards
  Path: resources/engineering-standards.md
  Trigger: OnActivation
  Render: Reference
  Template inputs: none
  Nested resources: none
  Dependency-pending inputs: none
  Unresolved inputs: none
  Unused inputs: none
  Body digest: none
  Inclusion reason: activation-triggered
  Evidence: digest=sha256:cb6c8cef844ef59d721d8bca1f4e78c43db732df6b774bdd06251a581101355e, byte-size=3378, binary=false
  Static note: resource bodies are supplied separately by IO/runtime evidence; static hosts may only see this contract.
- resource:gate-conventions
  Path: resources/gate-conventions.md
  Trigger: OnActivation
  Render: Reference
  Template inputs: none
  Nested resources: none
  Dependency-pending inputs: none
  Unresolved inputs: none
  Unused inputs: none
  Body digest: none
  Inclusion reason: activation-triggered
  Evidence: digest=sha256:5727e68d99977fc5986d1d99495a5d615dfb61a2116bcd6ee5633c4fa29471bc, byte-size=2429, binary=false
  Static note: resource bodies are supplied separately by IO/runtime evidence; static hosts may only see this contract.

