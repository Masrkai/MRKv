# AGENTS.md - mrkv

## Commands
- **Run / Dev**: `cargo run` (or `cargo build`)
- **Test**: `cargo test`
- **Lint**: `cargo clippy`
- **Format**: `cargo fmt`
- **Verification Order**: `cargo fmt --check && cargo clippy && cargo test`
- **Development Shell**: `nix-shell` (sets up required native libraries, compilers, and toolchain).

## Architecture & Entrypoints
- **App Binary**: Rust project (`src/main.rs`) using the [Slint](https://slint.dev/) UI framework.
- **UI Markup**: Slint definition in `ui/mrkv.slint`, compiled via `build.rs` (`slint-build`).
- **Core Modules**:
  - `src/main.rs`: Application entrypoint, state management, worker threads for asset/mermaid rendering, and Slint callbacks.
  - `src/fs.rs`: Workspace file tree traversal and directory handling.
  - `src/markdown.rs`: Markdown parsing via `comrak` and AST block building.

## Framework & Toolchain Quirks
- **Wayland Only**: Slint is configured in `Cargo.toml` with `backend-winit-wayland` (X11 backend is not enabled). A Wayland environment is required for GUI execution.
- **Software Renderer**: Configured with `renderer-software` (CPU-rendered UI) in `Cargo.toml`.
- **Native Dependencies**: Relies on system libraries (`fontconfig`, `freetype`, `libxkbcommon`, `wayland`, `libGL`, `vulkan-loader`) provided by `shell.nix`.

## Documentation Maintenance
- **README Updates**: If code changes in an important way that makes something in `README.md` not true or requires notes/explanation from a usage perspective, you must update `README.md` accordingly.
