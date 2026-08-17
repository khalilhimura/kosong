//! `kosong mcp` — Model Context Protocol server over stdio.
//!
//! Lesson: software can describe its own capabilities to other software.

use super::Context;
use crate::exit::{CliError, CliResult};

pub fn run(_context: &Context) -> CliResult<()> {
    Err(CliError::usage(
        "NOT_IMPLEMENTED",
        "MCP server is not yet implemented in Phase 1.",
    ))
}
