//! Evaluation settings shared by the scene evaluator, the console and the
//! plot compiler.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use viscas_core::eval::{AngleUnit, EvalOptions, FormatOptions};
use viscas_core::number::Precision;
use viscas_core::syntax::{DefinitionOp, ParseOptions};

/// User-facing evaluation preferences. Changing them with
/// [`crate::Engine::set_settings`] re-evaluates the whole scene.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EngineSettings {
    /// Approx mode: the primary display is decimal ([`crate::NodeValue::display`]).
    /// Stored values always stay exact.
    pub approx: bool,
    /// Working precision of inexact operations, in significant digits.
    pub precision_digits: u32,
    /// Significant digits of decimal output.
    pub display_digits: usize,
    pub angle: AngleUnit,
    /// Complex mode (`√-1 = i` instead of a "not real" error).
    pub complex: bool,
    /// Definition operator of console input and expression-list lines.
    pub definition_op: DefinitionOp,
}

impl Default for EngineSettings {
    fn default() -> Self {
        EngineSettings {
            approx: false,
            precision_digits: 50,
            display_digits: 12,
            angle: AngleUnit::Radians,
            complex: false,
            definition_op: DefinitionOp::ColonEq,
        }
    }
}

impl EngineSettings {
    /// Evaluator options (exact arithmetic; approx only affects display).
    pub fn eval_options(&self, interrupt: Option<Arc<AtomicBool>>) -> EvalOptions {
        EvalOptions {
            approx: false,
            precision: Precision::digits(self.precision_digits),
            angle: self.angle,
            complex: self.complex,
            interrupt,
            ..EvalOptions::default()
        }
    }

    /// Parser options for console input and expression-list lines.
    pub fn parse_options(&self) -> ParseOptions {
        ParseOptions {
            definition_op: self.definition_op,
        }
    }

    /// Exact-style formatting (`1/3`, `≈3.14159265359`).
    pub fn exact_format(&self) -> FormatOptions {
        FormatOptions {
            digits: self.display_digits,
            ..FormatOptions::default()
        }
    }

    /// Decimal formatting.
    pub fn approx_format(&self) -> FormatOptions {
        FormatOptions::approx(self.display_digits)
    }

    /// Short description, e.g. `exact · rad · real`.
    pub fn mode_summary(&self) -> String {
        format!(
            "{} · {} · {}",
            if self.approx { "approx" } else { "exact" },
            match self.angle {
                AngleUnit::Radians => "rad",
                AngleUnit::Degrees => "deg",
            },
            if self.complex { "complex" } else { "real" }
        )
    }
}
