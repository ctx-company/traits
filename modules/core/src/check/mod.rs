//! Check reports: combine validation, audit, activation, lock, resource, and
//! render-readiness information into a single reviewable report.
//!
//! `ctx traits check --locked` detects source, model-view, policy/resource, and
//! export drift once those artifacts exist. This module produces the report
//! shape; it does not execute agent behavior or product evals.

mod report;
mod section;
mod warning;

pub use report::CheckReport;
pub use section::{CheckSection, Section};
pub use warning::{CheckWarning, family_variant_advisories, resource_root_advisories};
