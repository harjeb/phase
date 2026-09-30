//! Structural combo recognition and bounded action-chain planning.
//!
//! Default templates recognize effect roles in live card definitions. The
//! root planner validates complete traces through engine-issued candidates
//! and simulation; policies reward only the next action of a current plan.
//! Opponents pass in this simulation. A completed cycle is a one-cycle
//! witness, not a guarantee of an unbounded or interaction-proof win.

pub mod components;
pub mod detection;
pub mod line;
pub mod planning;
pub mod registry;

pub use detection::{ComboDetector, StructuralComboDetector};
pub use line::{
    CardPredicate, ComboLine, ComboLineId, ComboPiece, ComboReachability, ComboStep, ComponentRole,
    WinKind,
};
pub use planning::{plan_combos, ComboPlan, ComboPlanOutcome, ComboPlanningResult, PlannedAction};
pub use registry::ComboRegistry;

#[cfg(test)]
pub(crate) mod tests;
