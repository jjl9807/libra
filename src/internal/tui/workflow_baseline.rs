//! Re-export of the runtime-owned W0-02 workflow baseline.
//!
//! Ownership moved to [`crate::internal::ai::workflow_baseline`] so Code UI
//! Web harness tests do not depend on `src/internal/tui/` as relevant source
//! (plan-20260715 W3-02).

pub use crate::internal::ai::workflow_baseline::*;
