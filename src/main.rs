slint::include_modules!();

mod fs;
mod markdown;

use clap::Parser;
use slint::{
    ComponentHandle, Image, Model, ModelRc, Rgba8Pixel, SharedPixelBuffer, StyledText, VecModel,
};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const ASSET_WORKERS: usize = 4;
/// Images larger than this (in either dimension) are downscaled in the worker.
const MAX_IMAGE_DIMENSION: u32 = 2000;

#[derive(Parser, Debug)]
#[command(name = "mrkv", author, version, about = "Linux native Markdown previewer")]
struct Cli {
    /// Path to a markdown file or directory to open
    path: Option<PathBuf>,
}

#[derive(Clone)]
struct RenderState {
    generation: Rc<Cell<u64>>,
    current_model: Rc<RefCell<Option<Rc<VecModel<MarkdownBlock>>>>>,
    job_tx: Sender<AssetJob>,
}

enum AssetJob {
    Image {
        generation: u64,
        index: usize,
        /// `Some(i)` when the image belongs to an image row (block type 61).
        sub: Option<usize>,
        source: String,
    },
    Mermaid {
        generation: u64,
        index: usize,
        source: String,
        dark: bool,
    },
}

struct AssetResult {
    generation: u64,
    index: usize,
    sub: Option<usize>,
    result: Result<AssetPayload, String>,
}

#[derive(Clone)]
enum AssetPayload {
    Rgba {
        width: u32,
        height: u32,
        pixels: Vec<u8>,
    },
    Svg(Vec<u8>),
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    let initial_path = cli.path.unwrap_or_else(|| PathBuf::from("."));

    let (root_dir, initial_file, has_parent) = if initial_path.is_file() {
        let parent = initial_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        (parent, Some(initial_path), true)
    } else if initial_path.is_dir() {
        let has_p = initial_path.parent().is_some()
            && initial_path != Path::new("/")
            && !initial_path.as_os_str().is_empty();
        (initial_path, None, has_p)
    } else {
        (PathBuf::from("."), None, false)
    };

    let main_window = MainWindow::new()?;
    main_window.set_has_parent_folder(has_parent);
    main_window.set_current_workspace_path(root_dir.to_string_lossy().to_string().into());

    let tree_state = Rc::new(RefCell::new(fs::FileTreeState::new(root_dir.clone())));

    let (job_tx, job_rx) = mpsc::channel::<AssetJob>();
    let (result_tx, result_rx) = mpsc::channel::<AssetResult>();

    let image_cache = Arc::new(Mutex::new(HashMap::<String, AssetPayload>::new()));
    let mermaid_cache = Arc::new(Mutex::new(HashMap::<String, AssetPayload>::new()));

    // A small pool: one slow download no longer blocks other images or Mermaid renders.
    let job_rx = Arc::new(Mutex::new(job_rx));
    let http_agent = ureq::AgentBuilder::new()
        .user_agent("mrkv/0.1 (markdown previewer)")
        .timeout_connect(Duration::from_secs(8))
        .timeout(Duration::from_secs(20))
        .build();

    for worker in 0..ASSET_WORKERS {
        let job_rx = Arc::clone(&job_rx);
        let result_tx = result_tx.clone();
        let image_cache = Arc::clone(&image_cache);
        let mermaid_cache = Arc::clone(&mermaid_cache);
        let agent = http_agent.clone();

        std::thread::Builder::new()
            .name(format!("mrkv-assets-{}", worker))
            .spawn(move || {
                asset_worker(job_rx, result_tx, agent, image_cache, mermaid_cache);
            })?;
    }
    drop(result_tx);

    let render_state = RenderState {
        generation: Rc::new(Cell::new(0)),
        current_model: Rc::new(RefCell::new(None)),
        job_tx,
    };

    // Helper to refresh tree view in UI
    let update_tree_ui = {
        let window_weak = main_window.as_weak();
        let tree_state = Rc::clone(&tree_state);
        move || {
            if let Some(win) = window_weak.upgrade() {
                let state = tree_state.borrow();
                let visible_nodes = state.build_visible_list();

                let slint_nodes: Vec<TreeNode> = visible_nodes
                    .into_iter()
                    .map(|(info, indent)| {
                        let is_selected = state
                            .selected_path
                            .as_ref()
                            .map_or(false, |p| p.to_string_lossy() == info.path);
                        TreeNode {
                            text: info.text.into(),
                            path: info.path.into(),
                            is_dir: info.is_dir,
                            is_expanded: info.is_expanded,
                            indent,
                            is_selected,
                        }
                    })
                    .collect();

                win.set_tree_nodes(ModelRc::new(VecModel::from(slint_nodes)));
            }
        }
    };

    update_tree_ui();

    // Helper to open and render a markdown file
    let open_file = {
        let window_weak = main_window.as_weak();
        let tree_state = Rc::clone(&tree_state);
        let render_state = render_state.clone();

        move |path: &Path, pending_anchor: Option<&str>| {
            if let Some(win) = window_weak.upgrade() {
                render_file(&win, &tree_state, &render_state, path, pending_anchor);
            }
        }
    };

    // If an initial file was specified, open it
    if let Some(file_path) = initial_file {
        open_file(&file_path, None);
    } else {
        // Try to find a README.md in root
        let readme_path = root_dir.join("README.md");
        if readme_path.exists() {
            open_file(&readme_path, None);
        }
    }

    // Callbacks
    {
        let open_file_clone = open_file.clone();
        let update_tree_clone = update_tree_ui.clone();
        let tree_state_clone = Rc::clone(&tree_state);
        main_window.on_select_node(move |path_str| {
            let path = PathBuf::from(path_str.as_str());
            if path.is_file() {
                open_file_clone(&path, None);
                update_tree_clone();
            } else if path.is_dir() {
                let mut state = tree_state_clone.borrow_mut();
                state.selected_path = Some(path);
                drop(state);
                update_tree_clone();
            }
        });
    }

    {
        let update_tree_clone = update_tree_ui.clone();
        let tree_state_clone = Rc::clone(&tree_state);
        main_window.on_toggle_folder(move |path_str| {
            let path = PathBuf::from(path_str.as_str());
            tree_state_clone.borrow_mut().toggle_expand(&path);
            update_tree_clone();
        });
    }

    {
        let window_weak = main_window.as_weak();
        let tree_state_clone = Rc::clone(&tree_state);
        let update_tree_clone = update_tree_ui.clone();
        main_window.on_open_folder_dialog(move || {
            if let Some(win) = window_weak.upgrade() {
                let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
                let new_root = PathBuf::from(home);
                let has_p = new_root.parent().is_some() && new_root != Path::new("/");
                {
                    let mut state = tree_state_clone.borrow_mut();
                    state.root_path = new_root.clone();
                    state.expanded_dirs.clear();
                    state.expanded_dirs.insert(new_root.clone());
                    state.selected_path = None;
                }
                win.set_current_workspace_path(new_root.to_string_lossy().to_string().into());
                win.set_has_parent_folder(has_p);
                update_tree_clone();
            }
        });
    }

    {
        let window_weak = main_window.as_weak();
        let tree_state_clone = Rc::clone(&tree_state);
        let update_tree_clone = update_tree_ui.clone();
        main_window.on_open_parent_folder(move || {
            let current_root = tree_state_clone.borrow().root_path.clone();
            if let Some(parent) = current_root.parent() {
                let parent_path = parent.to_path_buf();
                {
                    let mut state = tree_state_clone.borrow_mut();
                    state.root_path = parent_path.clone();
                    state.expanded_dirs.clear();
                    state.expanded_dirs.insert(parent_path.clone());
                    state.expanded_dirs.insert(current_root.clone());
                }
                if let Some(win) = window_weak.upgrade() {
                    win.set_current_workspace_path(
                        parent_path.to_string_lossy().to_string().into(),
                    );
                }
                update_tree_clone();
            }
        });
    }

    {
        let window_weak = main_window.as_weak();
        let render_state_clone = render_state.clone();
        let open_file_clone = open_file.clone();
        let update_tree_clone = update_tree_ui.clone();

        main_window.on_link_clicked(move |link| {
            if let Some(win) = window_weak.upgrade() {
                handle_link(&win, &render_state_clone, &open_file_clone, link.as_str());
                update_tree_clone();
            }
        });
    }

    {
        let window_weak = main_window.as_weak();
        let render_state_clone = render_state.clone();

        main_window.on_theme_changed(move || {
            if let Some(win) = window_weak.upgrade() {
                let dark = win.global::<Theme>().get_dark();
                rerender_mermaid(&win, &render_state_clone, dark);
            }
        });
    }

    let timer = slint::Timer::default();
    {
        let window_weak = main_window.as_weak();
        let current_model = Rc::clone(&render_state.current_model);
        let generation = Rc::clone(&render_state.generation);

        timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(50),
            move || loop {
                match result_rx.try_recv() {
                    Ok(result) => {
                        if result.generation != generation.get() {
                            continue;
                        }

                        let Some(win) = window_weak.upgrade() else {
                            break;
                        };

                        let model = current_model.borrow().clone();
                        let Some(model) = model else {
                            continue;
                        };

                        if result.index >= model.row_count() {
                            continue;
                        }

                        let Some(mut block) = model.row_data(result.index) else {
                            continue;
                        };

                        match result.sub {
                            Some(sub) => {
                                // Image inside a row: update just that item.
                                if let Some(mut item) = block.images.row_data(sub) {
                                    apply_image_item_result(&mut item, result.result);
                                    block.images.set_row_data(sub, item);
                                }
                            }
                            None => {
                                apply_asset_result(&win, &mut block, result.result);
                                model.set_row_data(result.index, block);
                            }
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => break,
                }
            },
        );
    }

    main_window.run()?;
    drop(timer);
    Ok(())
}

fn render_file(
    window: &MainWindow,
    tree_state: &Rc<RefCell<fs::FileTreeState>>,
    render_state: &RenderState,
    path: &Path,
    pending_anchor: Option<&str>,
) {
    let generation = render_state.generation.get().wrapping_add(1);
    render_state.generation.set(generation);

    match std::fs::read_to_string(path) {
        Ok(content) => {
            let parsed_blocks = markdown::parse_markdown(&content);
            let base_dir = path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("."));
            let dark = window.global::<Theme>().get_dark();

            let mut slint_blocks = Vec::with_capacity(parsed_blocks.len());
            let mut jobs = Vec::new();

            for (index, parsed) in parsed_blocks.into_iter().enumerate() {
                let rich = if parsed.rich.is_empty() {
                    StyledText::from_plain_text(&parsed.content)
                } else {
                    StyledText::from_markdown(&parsed.rich)
                        .unwrap_or_else(|_| StyledText::from_plain_text(&parsed.content))
                };

                let mut asset_state = 0;
                let mut asset_error = String::new();
                let mut row_images: Vec<ImageItem> = Vec::new();

                if parsed.block_type == 61 {
                    for (sub, img) in parsed.images.iter().enumerate() {
                        let mut item = ImageItem {
                            image: Image::default(),
                            alt: img.alt.clone().into(),
                            link: img.link.clone().into(),
                            asset_state: 0,
                            asset_error: "".into(),
                            width_hint: img.width,
                        };

                        match resolve_image_source(&img.src, &base_dir) {
                            Some(source) => {
                                item.asset_state = 1;
                                jobs.push(AssetJob::Image {
                                    generation,
                                    index,
                                    sub: Some(sub),
                                    source,
                                });
                            }
                            None => {
                                item.asset_state = 3;
                                item.asset_error = "Unsupported image source".into();
                            }
                        }

                        row_images.push(item);
                    }
                } else if parsed.block_type == 60 {
                    if parsed.image_src.trim().is_empty() {
                        asset_state = 3;
                        asset_error = "Missing image source".to_string();
                    } else if let Some(source) =
                        resolve_image_source(&parsed.image_src, &base_dir)
                    {
                        asset_state = 1;
                        jobs.push(AssetJob::Image {
                            generation,
                            index,
                            sub: None,
                            source,
                        });
                    } else {
                        asset_state = 3;
                        asset_error = "Unsupported image source".to_string();
                    }
                } else if parsed.block_type == 70 {
                    asset_state = 1;
                    jobs.push(AssetJob::Mermaid {
                        generation,
                        index,
                        source: parsed.content.clone(),
                        dark,
                    });
                }

                slint_blocks.push(MarkdownBlock {
                    block_type: parsed.block_type,
                    content: parsed.content.into(),
                    extra: parsed.extra.into(),
                    rich,
                    image: Image::default(),
                    image_alt: parsed.image_alt.into(),
                    link: parsed.link.into(),
                    asset_state,
                    asset_error: asset_error.into(),
                    anchor: parsed.anchor.into(),
                    indent: parsed.indent,
                    images: ModelRc::new(VecModel::from(row_images)),
                    center: parsed.center,
                    image_width_hint: parsed.image_width,
                });
            }

            let model = Rc::new(VecModel::from(slint_blocks));
            window.set_markdown_blocks(ModelRc::from(model.clone()));
            window.set_current_file_path(path.to_string_lossy().to_string().into());
            *render_state.current_model.borrow_mut() = Some(model);

            for job in jobs {
                let _ = render_state.job_tx.send(job);
            }

            tree_state.borrow_mut().selected_path = Some(path.to_path_buf());

            window.invoke_reset_scroll();

            if let Some(anchor) = pending_anchor.filter(|anchor| !anchor.is_empty()) {
                schedule_anchor_scroll(window, render_state, anchor.to_string());
            }
        }
        Err(err) => {
            eprintln!("Failed to read file {}: {}", path.display(), err);

            let message = format!("Error reading file: {}", err);
            let model = Rc::new(VecModel::from(vec![MarkdownBlock {
                block_type: 10,
                content: message.clone().into(),
                extra: "".into(),
                rich: StyledText::from_plain_text(&message),
                image: Image::default(),
                image_alt: "".into(),
                link: "".into(),
                asset_state: 0,
                asset_error: "".into(),
                anchor: "".into(),
                indent: 0,
                images: ModelRc::new(VecModel::from(Vec::<ImageItem>::new())),
                center: false,
                image_width_hint: 0,
            }]));

            window.set_markdown_blocks(ModelRc::from(model.clone()));
            window.set_current_file_path(path.to_string_lossy().to_string().into());
            *render_state.current_model.borrow_mut() = Some(model);
            window.invoke_reset_scroll();
        }
    }
}

fn handle_link<F>(
    window: &MainWindow,
    render_state: &RenderState,
    open_file: &F,
    link: &str,
) where
    F: Fn(&Path, Option<&str>),
{
    let link = link.trim();

    if link.is_empty()
        || link.starts_with("http://")
        || link.starts_with("https://")
        || link.starts_with("mailto:")
        || link.starts_with("tel:")
    {
        return;
    }

    if let Some(anchor) = link.strip_prefix('#') {
        scroll_to_anchor(window, render_state, anchor);
        return;
    }

    let (target, fragment) = match link.split_once('#') {
        Some((target, fragment)) => (target, Some(fragment)),
        None => (link, None),
    };

    if target.is_empty() {
        if let Some(fragment) = fragment {
            scroll_to_anchor(window, render_state, fragment);
        }
        return;
    }

    let decoded = percent_encoding::percent_decode_str(target)
        .decode_utf8_lossy()
        .into_owned();
    let target = decoded.strip_prefix("file://").unwrap_or(&decoded);

    let current_path = PathBuf::from(window.get_current_file_path().to_string());
    let base_dir = current_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    let target_path = if Path::new(target).is_absolute() {
        PathBuf::from(target)
    } else {
        base_dir.join(target)
    };

    if is_markdown_path(&target_path) {
        if target_path.is_file() {
            open_file(&target_path, fragment);
        }
    } else if target_path.is_file() {
        let _ = std::process::Command::new("xdg-open")
            .arg(&target_path)
            .spawn();
    }
}

fn is_markdown_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| {
            extension.eq_ignore_ascii_case("md") || extension.eq_ignore_ascii_case("markdown")
        })
        .unwrap_or(false)
}

fn scroll_to_anchor(window: &MainWindow, render_state: &RenderState, anchor: &str) {
    let model = render_state.current_model.borrow().clone();
    let Some(model) = model else {
        return;
    };

    for index in 0..model.row_count() {
        let Some(block) = model.row_data(index) else {
            continue;
        };

        if block.anchor.as_str() == anchor {
            window.invoke_scroll_to_block(index as i32);
            return;
        }
    }
}

fn schedule_anchor_scroll(window: &MainWindow, render_state: &RenderState, anchor: String) {
    let window_weak = window.as_weak();
    let current_model = Rc::clone(&render_state.current_model);

    slint::Timer::single_shot(Duration::from_millis(120), move || {
        let Some(window) = window_weak.upgrade() else {
            return;
        };
        let model = current_model.borrow().clone();
        let Some(model) = model else {
            return;
        };

        for index in 0..model.row_count() {
            let Some(block) = model.row_data(index) else {
                continue;
            };

            if block.anchor.as_str() == anchor {
                window.invoke_scroll_to_block(index as i32);
                return;
            }
        }
    });
}

fn resolve_image_source(raw: &str, base_dir: &Path) -> Option<String> {
    let raw = raw.trim();

    if raw.is_empty() || raw.starts_with("data:") {
        return None;
    }

    if raw.starts_with("http://") || raw.starts_with("https://") || raw.starts_with("file://") {
        return Some(raw.to_string());
    }

    if raw.starts_with("//") {
        return Some(format!("https:{}", raw));
    }

    let decoded = percent_encoding::percent_decode_str(raw)
        .decode_utf8_lossy()
        .into_owned();
    let path = if Path::new(&decoded).is_absolute() {
        PathBuf::from(decoded)
    } else {
        base_dir.join(decoded)
    };

    Some(path.to_string_lossy().to_string())
}

fn rerender_mermaid(window: &MainWindow, render_state: &RenderState, dark: bool) {
    let model = render_state.current_model.borrow().clone();
    let Some(model) = model else {
        return;
    };

    let generation = render_state.generation.get();

    for index in 0..model.row_count() {
        let Some(mut block) = model.row_data(index) else {
            continue;
        };

        if block.block_type != 70 {
            continue;
        }

        block.asset_state = 1;
        block.asset_error = "".into();
        block.image = Image::default();
        model.set_row_data(index, block.clone());

        let _ = render_state.job_tx.send(AssetJob::Mermaid {
            generation,
            index,
            source: block.content.to_string(),
            dark,
        });
    }

    let _ = window;
}

fn payload_to_image(result: Result<AssetPayload, String>) -> Result<Image, String> {
    match result? {
        AssetPayload::Rgba {
            width,
            height,
            pixels,
        } => {
            let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(width, height);
            buffer.make_mut_bytes().copy_from_slice(&pixels);
            Ok(Image::from_rgba8(buffer))
        }
        AssetPayload::Svg(bytes) => Image::load_from_svg_data(&bytes)
            .map_err(|_| "Failed to decode SVG image".to_string()),
    }
}

fn apply_asset_result(
    _window: &MainWindow,
    block: &mut MarkdownBlock,
    result: Result<AssetPayload, String>,
) {
    match payload_to_image(result) {
        Ok(image) => {
            block.image = image;
            block.asset_state = 2;
            block.asset_error = "".into();
        }
        Err(error) => {
            block.asset_state = 3;
            block.asset_error = error.into();
        }
    }
}

fn apply_image_item_result(item: &mut ImageItem, result: Result<AssetPayload, String>) {
    match payload_to_image(result) {
        Ok(image) => {
            item.image = image;
            item.asset_state = 2;
            item.asset_error = "".into();
        }
        Err(error) => {
            item.asset_state = 3;
            item.asset_error = error.into();
        }
    }
}

fn job_ids(job: &AssetJob) -> (u64, usize, Option<usize>) {
    match job {
        AssetJob::Image {
            generation,
            index,
            sub,
            ..
        } => (*generation, *index, *sub),
        AssetJob::Mermaid {
            generation, index, ..
        } => (*generation, *index, None),
    }
}

fn process_job(
    job: AssetJob,
    agent: &ureq::Agent,
    image_cache: &Arc<Mutex<HashMap<String, AssetPayload>>>,
    mermaid_cache: &Arc<Mutex<HashMap<String, AssetPayload>>>,
) -> AssetResult {
    match job {
        AssetJob::Image {
            generation,
            index,
            sub,
            source,
        } => AssetResult {
            generation,
            index,
            sub,
            result: load_image(&source, agent, image_cache),
        },
        AssetJob::Mermaid {
            generation,
            index,
            source,
            dark,
        } => AssetResult {
            generation,
            index,
            sub: None,
            result: render_mermaid(&source, dark, mermaid_cache),
        },
    }
}

fn asset_worker(
    job_rx: Arc<Mutex<Receiver<AssetJob>>>,
    result_tx: Sender<AssetResult>,
    agent: ureq::Agent,
    image_cache: Arc<Mutex<HashMap<String, AssetPayload>>>,
    mermaid_cache: Arc<Mutex<HashMap<String, AssetPayload>>>,
) {
    loop {
        // Only one worker waits in recv() at a time; the lock is released
        // as soon as a job arrives so the others can pick up the next one.
        let job = {
            let Ok(receiver) = job_rx.lock() else {
                break;
            };
            match receiver.recv() {
                Ok(job) => job,
                Err(_) => break,
            }
        };

        let (generation, index, sub) = job_ids(&job);

        // A panic in a decoder/renderer must not kill the worker.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            process_job(job, &agent, &image_cache, &mermaid_cache)
        }))
        .unwrap_or_else(|_| AssetResult {
            generation,
            index,
            sub,
            result: Err("Renderer panicked".to_string()),
        });

        if result_tx.send(result).is_err() {
            break;
        }
    }
}

fn load_image(
    source: &str,
    agent: &ureq::Agent,
    cache: &Arc<Mutex<HashMap<String, AssetPayload>>>,
) -> Result<AssetPayload, String> {
    if let Some(cached) = cache.lock().ok().and_then(|cache| cache.get(source).cloned()) {
        return Ok(cached);
    }

    let (bytes, svg_hint) = if source.starts_with("http://") || source.starts_with("https://") {
        let response = agent
            .get(source)
            .call()
            .map_err(|err| format!("Failed to download image: {}", err))?;
        let svg_hint = response
            .header("content-type")
            .map(|content_type| content_type.to_ascii_lowercase().contains("svg"))
            .unwrap_or(false)
            || source.to_ascii_lowercase().contains(".svg");

        let mut bytes = Vec::new();
        response
            .into_reader()
            .take(32 * 1024 * 1024)
            .read_to_end(&mut bytes)
            .map_err(|err| format!("Failed to read image: {}", err))?;

        (bytes, svg_hint)
    } else {
        let path = source.strip_prefix("file://").unwrap_or(source);
        let svg_hint = Path::new(path)
            .extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| extension.eq_ignore_ascii_case("svg"))
            .unwrap_or(false);
        let bytes = std::fs::read(path)
            .map_err(|err| format!("Failed to read image {}: {}", path, err))?;

        (bytes, svg_hint)
    };

    let payload = if svg_hint || looks_like_svg(&bytes) {
        AssetPayload::Svg(bytes)
    } else {
        let mut image = image::load_from_memory(&bytes)
            .map_err(|err| format!("Failed to decode image: {}", err))?;

        // Keep memory and upload cost bounded for huge screenshots / photos.
        if image.width().max(image.height()) > MAX_IMAGE_DIMENSION {
            image = image.resize(
                MAX_IMAGE_DIMENSION,
                MAX_IMAGE_DIMENSION,
                image::imageops::FilterType::Triangle,
            );
        }

        let rgba = image.to_rgba8();
        let (width, height) = rgba.dimensions();
        AssetPayload::Rgba {
            width,
            height,
            pixels: rgba.into_raw(),
        }
    };

    if let Ok(mut cache) = cache.lock() {
        cache.insert(source.to_string(), payload.clone());
    }

    Ok(payload)
}

fn looks_like_svg(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(1024)];
    String::from_utf8_lossy(head)
        .to_ascii_lowercase()
        .contains("<svg")
}

fn render_mermaid(
    source: &str,
    dark: bool,
    cache: &Arc<Mutex<HashMap<String, AssetPayload>>>,
) -> Result<AssetPayload, String> {
    let cache_key = format!("{}:{}", dark, source);

    if let Some(cached) = cache
        .lock()
        .ok()
        .and_then(|cache| cache.get(&cache_key).cloned())
    {
        return Ok(cached);
    }

    let mut options = mermaid_rs_renderer::RenderOptions::default();
    options.theme = if dark {
        mermaid_rs_renderer::Theme::dark()
    } else {
        mermaid_rs_renderer::Theme::modern()
    };
    options.theme.background = if dark { "#121418" } else { "#faf9f6" }.to_string();

    let svg = mermaid_rs_renderer::render_with_options(source, options)
        .map_err(|err| format!("Mermaid render failed: {}", err))?;
    let payload = AssetPayload::Svg(svg.into_bytes());

    if let Ok(mut cache) = cache.lock() {
        cache.insert(cache_key, payload.clone());
    }

    Ok(payload)
}