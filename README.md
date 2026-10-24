# MRKv

MRKv is a fast, Linux-native Markdown previewer and workspace explorer written in Rust, powered by the [Slint](https://slint.dev/) UI framework and `comrak` markdown parser.

## Features

- **Native Linux UI**: Built with Slint (configured with Wayland backend and software rendering support).
- **Workspace File Tree**: Sidebar file explorer to browse directories and switch between markdown documents instantly.
- **Rich Markdown Rendering**: Parses Markdown via `comrak` with support for headings, lists, blockquotes, code blocks, tables, task lists, and more.
- **Asynchronous Asset Loading**: Background worker threads handle image downscaling/loading and Mermaid diagram rendering without blocking the UI.
- **Responsive Performance**: Incremental block loading with bounded UI frame budgets for smooth scrolling and rendering.

## Prerequisites

MRKv requires system libraries for windowing, fonts, and rendering (Wayland, Fontconfig, FreeType, libxkbcommon, libGL, Vulkan). 

If you are using Nix, a `shell.nix` environment is provided that sets up all necessary native dependencies, compilers, and toolchains automatically:

```bash
nix-shell
```

## Getting Started

### Build & Run

```bash
# Enter development shell (if using Nix)
nix-shell

# Run MRKv (optionally specifying a file or directory path)
cargo run -- [PATH]
```

### Development Commands

- **Run / Dev**: `cargo run` (or `cargo build`)
- **Test**: `cargo test`
- **Lint**: `cargo clippy`
- **Format**: `cargo fmt`
- **Verification Order**: `cargo fmt --check && cargo clippy && cargo test`

## Architecture Overview

- `src/main.rs`: Application entrypoint, CLI parsing (`clap`), state management, worker threads for asset/mermaid rendering, and Slint UI callbacks.
- `src/fs.rs`: Workspace file tree traversal and directory handling.
- `src/markdown.rs`: Markdown parsing via `comrak` and AST block building.
- `ui/mrkv.slint`: Slint UI markup and component definitions.
