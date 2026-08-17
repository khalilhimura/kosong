# Task 2 Report: MCP Server with Read-Only Tools

## Summary

Implemented the MCP server in `commands/mcp.rs` using the `rmcp` SDK, exposing three read-only tools over stdio transport. Also added `show_raw()` to `commands/show.rs` and made `Context` cloneable by wrapping its `Runtime` in `Arc`.

## Files Changed

### 1. `crates/kosong-cli/src/commands/mcp.rs` — Full replacement

The skeleton from Task 1 was replaced with a complete MCP server implementation. The `KosongMcp` struct holds its own `ToolRouter` and an `Arc<Context>` (since `Context` is now `Clone`).

```diff
-//! `kosong mcp` — Model Context Protocol server over stdio.
-//!
-//! Lesson: software can describe its own capabilities to other software.
-
-use super::Context;
-use crate::exit::{CliError, CliResult};
-
-pub fn run(_context: &Context) -> CliResult<()> {
-    Err(CliError::usage(
-        "NOT_IMPLEMENTED",
-        "MCP server is not yet implemented in Phase 1.",
-    ))
-}
+//! (full implementation — see file for 97 lines)
```

Key additions:
- `KosongMcp` struct with `ToolRouter` + `Arc<Context>`
- `#[tool_router]` impl block with three async tools:
  - `kosong_status` — wraps `status_json()`, returns JSON string
  - `kosong_doctor` — wraps `doctor_json()`, returns JSON string
  - `kosong_show` — wraps `show::show_raw()`, returns text content
- `#[tool_handler]` impl for `ServerHandler` with tool capabilities
- `run()` function using a fresh tokio current-thread runtime + stdio transport

All tool errors are caught and returned as string content — the server never crashes from a tool error.

### 2. `crates/kosong-cli/src/commands/show.rs` — Added `show_raw()`

```diff
+/// Returns the raw document text without printing it.
+pub fn show_raw(context: &Context) -> CliResult<String> {
+    let workspace = context.workspace()?;
+    let path = workspace.document_path();
+    read_checked(path)
+        .map(|s| s.trim_end().to_owned())
+        .map_err(|e| {
+            CliError::internal(
+                "SHOW_RAW_FAILED",
+                format!("could not read document: {e}"),
+            )
+            .with_repair(e.repair())
+        })
+}
```

This function extracts the raw-content logic from `show --raw` so the MCP server can call it without printing to stdout. The existing `run()` function still uses `ui.always()` for interactive use.

### 3. `crates/kosong-cli/src/commands/mod.rs` — Made `Context` cloneable

Two changes:
- Added `#[derive(Clone)]` to `Context`
- Changed `runtime: tokio::runtime::Runtime` → `runtime: Arc<tokio::runtime::Runtime>`

The `Context::new()` constructor wraps the runtime in `Arc::new(runtime)`. The `block_on` method works identically through `Arc` deref. `Ui` was already `Copy + Clone`, and `Utf8PathBuf` is `Clone`, making all fields cloneable.

## Build Results

| Check | Result |
|---|---|
| `cargo build` | Only pre-existing `site` module collision (module declared via both `site.rs` and `site/mod.rs`) |
| Our code | Compiles cleanly — no warnings, no errors |
| `cargo clippy` | Pre-existing errors only; our code adds no warnings |
| `cargo test -p kosong-core` | All 223 tests pass ✅ |

### Pre-existing errors (not introduced by this task)

1. **`site` module collision** — `crates/kosong-cli/src/commands/site.rs` and `crates/kosong-cli/src/commands/site/mod.rs` both exist
2. Missing `publish` and `rollback` modules in `site/`
3. Missing `gh_repair` / `wrangler_repair` functions in provider modules

These are all in the `site/` directory and unrelated to this task.

## Test Results

- `kosong-core` unit tests: **223 passed** ✅
- `kosong-cli` integration tests: Could not run due to pre-existing `site` collision preventing binary compilation

## Concerns

1. **Pre-existing `site` collision**: The working tree has uncommitted `site/` directory that conflicts with `site.rs`. This will need to be resolved before the binary can be built or integration tests run.
2. **`Arc<Context>` ownership**: The `run()` function calls `context.clone()` (now possible with `#[derive(Clone)]`) and wraps the clone in `Arc`. This creates a small overhead of one Arc allocation per server start, which is negligible for a long-running stdio server.
3. **Separate tokio runtime**: MCP's `run()` creates its own tokio current-thread runtime separate from the one inside `Context`. This is intentional — the MCP runtime is dedicated to the stdio transport, while the `Context` runtime is for CLI provider calls.
