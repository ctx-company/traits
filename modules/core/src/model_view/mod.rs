//! Model-visible compilation: compile normalized traits into deterministic
//! sanitized model-visible text.
//!
//! Model-visible text is the compiled representation that may be injected into
//! a host model's context. The behavior envelope includes only summary, intent,
//! behavior, and selected resources; the full authoring envelope adds reviewed
//! procedure and boundary data. Both remove or replace hidden/deceptive
//! constructs before they reach compiled text and record every normalization.

include!("types.rs");
include!("emit.rs");
include!("compile.rs");
include!("compile_sections.rs");
include!("sanitize.rs");
