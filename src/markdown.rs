use comrak::nodes::{AstNode, ListType, NodeValue};
use comrak::{parse_document, Arena, Options};

/// Block types understood by the UI:
/// 1-6 headings, 10 paragraph, 20 code, 30 list item, 40 blockquote,
/// 50 rule, 60 single image, 61 image row (badges etc.), 70 mermaid.
#[derive(Debug, Clone)]
pub struct ParsedImage {
    pub src: String,
    pub alt: String,
    pub link: String,
    /// Width from an HTML `width="..."` attribute, 0 when unspecified.
    pub width: i32,
}

#[derive(Debug, Clone)]
pub struct ParsedCell {
    pub rich: String,
    pub plain: String,
}

#[derive(Debug, Clone)]
pub struct ParsedRow {
    pub header: bool,
    pub cells: Vec<ParsedCell>,
}

/// A block of Markdown ready to be handed to Slint.
///
/// `rich` contains inline Markdown (emphasis, links, code, ...) and is parsed by
/// Slint's `StyledText` at runtime. `content` is the plain-text fallback.
#[derive(Debug, Clone)]
pub struct ParsedBlock {
    pub block_type: i32,
    pub content: String,
    pub extra: String,
    pub rich: String,
    pub image_src: String,
    pub image_alt: String,
    pub image_width: i32,
    pub images: Vec<ParsedImage>,
    /// Table rows (block type 80) and the column count.
    pub rows: Vec<ParsedRow>,
    pub cols: i32,
    pub link: String,
    pub anchor: String,
    pub indent: i32,
    pub center: bool,
}

impl ParsedBlock {
    fn new(block_type: i32) -> Self {
        Self {
            block_type,
            content: String::new(),
            extra: String::new(),
            rich: String::new(),
            image_src: String::new(),
            image_alt: String::new(),
            image_width: 0,
            images: Vec::new(),
            rows: Vec::new(),
            cols: 0,
            link: String::new(),
            anchor: String::new(),
            indent: 0,
            center: false,
        }
    }
}

#[derive(Debug, Clone)]
struct InlineRun {
    markdown: String,
    plain: String,
}

#[derive(Debug, Clone)]
enum InlinePiece {
    Text(InlineRun),
    Image {
        src: String,
        alt: String,
        link: String,
        width: i32,
    },
}

pub fn parse_markdown(markdown: &str) -> Vec<ParsedBlock> {
    let arena = Arena::new();
    let mut options = Options::default();
    options.extension.table = true;
    options.extension.strikethrough = true;
    options.extension.autolink = true;
    let root = parse_document(&arena, markdown, &options);

    let mut blocks = Vec::new();
    let mut counters = [0i32; 6];

    for node in root.children() {
        walk_block(node, &mut blocks, 0, &mut counters, false);
    }

    blocks
}

fn walk_block<'a>(
    node: &'a AstNode<'a>,
    blocks: &mut Vec<ParsedBlock>,
    list_depth: usize,
    counters: &mut [i32; 6],
    in_quote: bool,
) {
    let data = node.data.borrow();

    match &data.value {
        NodeValue::Heading(heading) => {
            let level = heading.level.clamp(1, 6) as usize;
            counters[level - 1] += 1;
            for counter in counters.iter_mut().skip(level) {
                *counter = 0;
            }

            let (rich, plain) = inline_content(node);
            let mut block = ParsedBlock::new(level as i32);
            block.content = plain;
            block.rich = if rich.is_empty() {
                rich
            } else {
                format!("**{}**", rich)
            };
            block.anchor = slugify(&block.content);
            blocks.push(block);
        }

        NodeValue::Paragraph => {
            let block_type = if in_quote { 40 } else { 10 };
            push_inline_blocks(node, blocks, block_type, "", 0);
        }

        NodeValue::CodeBlock(code) => {
            let info = code.info.trim().to_string();
            let language = info.split_whitespace().next().unwrap_or("");
            let mut block = ParsedBlock::new(if language.eq_ignore_ascii_case("mermaid") {
                70
            } else {
                20
            });
            block.content = code.literal.trim_end_matches('\n').to_string();
            block.extra = info;
            blocks.push(block);
        }

        NodeValue::BlockQuote => {
            for child in node.children() {
                walk_block(child, blocks, list_depth, counters, true);
            }
        }

        NodeValue::List(list) => {
            let ordered = matches!(&list.list_type, ListType::Ordered);
            let mut number = list.start;

            for item in node.children() {
                let marker = if ordered {
                    let marker = format!("{}.", number);
                    number += 1;
                    marker
                } else {
                    "•".to_string()
                };

                push_list_item(item, blocks, &marker, list_depth, counters, in_quote);
            }
        }

        NodeValue::Table(_) => {
            push_table(node, blocks);
        }

        NodeValue::ThematicBreak => {
            blocks.push(ParsedBlock::new(50));
        }

        // Raw HTML (very common in READMEs): extract images, links, headings,
        // alignment and plain text instead of dropping the whole block.
        NodeValue::HtmlBlock(html) => {
            html_to_blocks(&html.literal, blocks, if in_quote { 40 } else { 10 });
        }

        _ => {
            for child in node.children() {
                walk_block(child, blocks, list_depth, counters, in_quote);
            }
        }
    }
}

/// Table (block type 80): rows of cells, each cell holding inline Markdown.
fn push_table<'a>(node: &'a AstNode<'a>, blocks: &mut Vec<ParsedBlock>) {
    let mut rows = Vec::new();
    let mut cols = 0usize;

    for row in node.children() {
        let header = matches!(row.data.borrow().value, NodeValue::TableRow(true));
        let mut cells = Vec::new();

        for cell in row.children() {
            let (rich, plain) = pieces_to_text(&inline_pieces(cell, None));
            let rich = if header && !rich.is_empty() {
                format!("**{}**", rich)
            } else {
                rich
            };
            cells.push(ParsedCell { rich, plain });
        }

        cols = cols.max(cells.len());
        rows.push(ParsedRow { header, cells });
    }

    if rows.is_empty() {
        return;
    }

    let mut block = ParsedBlock::new(80);
    block.rows = rows;
    block.cols = cols as i32;
    blocks.push(block);
}

fn push_list_item<'a>(
    item: &'a AstNode<'a>,
    blocks: &mut Vec<ParsedBlock>,
    marker: &str,
    list_depth: usize,
    counters: &mut [i32; 6],
    in_quote: bool,
) {
    let mut first_paragraph = true;

    for child in item.children() {
        let value = child.data.borrow().value.clone();

        match value {
            NodeValue::Paragraph => {
                if first_paragraph {
                    push_inline_blocks(child, blocks, 30, marker, list_depth);
                    first_paragraph = false;
                } else {
                    push_inline_blocks(child, blocks, 10, "", list_depth);
                }
            }

            NodeValue::TaskItem(checked) => {
                let task_marker = if checked.is_some() { "☑" } else { "☐" };

                for task_child in child.children() {
                    let task_value = task_child.data.borrow().value.clone();

                    match task_value {
                        NodeValue::Paragraph => {
                            let item_marker = if first_paragraph { task_marker } else { "" };
                            push_inline_blocks(task_child, blocks, 30, item_marker, list_depth);
                            first_paragraph = false;
                        }
                        NodeValue::List(_) => {
                            walk_block(task_child, blocks, list_depth + 1, counters, in_quote)
                        }
                        _ => walk_block(task_child, blocks, list_depth + 1, counters, in_quote),
                    }
                }
            }

            NodeValue::List(_) => {
                walk_block(child, blocks, list_depth + 1, counters, in_quote);
            }

            _ => {
                walk_block(child, blocks, list_depth + 1, counters, in_quote);
            }
        }
    }
}

/// Convert a paragraph-like node into text blocks and image blocks.
/// Images are split out so the UI can lay them out independently; consecutive
/// images (badge rows) are grouped into a single row block.
fn push_inline_blocks<'a>(
    node: &'a AstNode<'a>,
    blocks: &mut Vec<ParsedBlock>,
    block_type: i32,
    marker: &str,
    indent: usize,
) {
    let pieces = inline_pieces(node, None);
    emit_pieces(pieces, block_type, marker, indent, false, blocks);
}

/// Turn a flat list of inline pieces into blocks.
///
/// * Text becomes `block_type` blocks (a heading level 1-6 gets heading styling).
/// * A single image becomes a type-60 block.
/// * Two or more images with only whitespace between them become one type-61 row.
fn emit_pieces(
    pieces: Vec<InlinePiece>,
    block_type: i32,
    marker: &str,
    indent: usize,
    center: bool,
    blocks: &mut Vec<ParsedBlock>,
) {
    let mut rich = String::new();
    let mut plain = String::new();
    let mut images: Vec<ParsedImage> = Vec::new();
    let mut marker = marker.to_string();

    for piece in pieces {
        match piece {
            InlinePiece::Text(run) => {
                if !images.is_empty() {
                    // Whitespace / line breaks between images do not split a row.
                    if run.plain.trim().is_empty() {
                        continue;
                    }
                    flush_images(&mut images, indent, center, blocks);
                }
                rich.push_str(&run.markdown);
                plain.push_str(&run.plain);
            }
            InlinePiece::Image {
                src,
                alt,
                link,
                width,
            } => {
                flush_text(
                    &mut rich,
                    &mut plain,
                    block_type,
                    &mut marker,
                    indent,
                    center,
                    blocks,
                );

                if src.trim().is_empty() {
                    continue;
                }

                images.push(ParsedImage {
                    src,
                    alt,
                    link,
                    width,
                });
            }
        }
    }

    flush_images(&mut images, indent, center, blocks);
    flush_text(
        &mut rich,
        &mut plain,
        block_type,
        &mut marker,
        indent,
        center,
        blocks,
    );
}

fn flush_text(
    rich: &mut String,
    plain: &mut String,
    block_type: i32,
    marker: &mut String,
    indent: usize,
    center: bool,
    blocks: &mut Vec<ParsedBlock>,
) {
    if rich.trim().is_empty() && plain.trim().is_empty() {
        rich.clear();
        plain.clear();
        return;
    }

    let mut block = ParsedBlock::new(block_type);
    block.content = plain.trim().to_string();
    block.rich = rich.trim().to_string();
    block.indent = indent as i32;
    block.center = center;

    if (1..=6).contains(&block_type) {
        block.rich = format!("**{}**", block.rich);
        block.anchor = slugify(&block.content);
    } else {
        // Only the first block of a list item carries the bullet / number.
        block.extra = std::mem::take(marker);
    }

    blocks.push(block);
    rich.clear();
    plain.clear();
}

fn flush_images(
    images: &mut Vec<ParsedImage>,
    indent: usize,
    center: bool,
    blocks: &mut Vec<ParsedBlock>,
) {
    match images.len() {
        0 => {}
        1 => {
            let image = images.remove(0);
            let mut block = ParsedBlock::new(60);
            block.content = image.alt.clone();
            block.image_src = image.src;
            block.image_alt = image.alt;
            block.image_width = image.width;
            block.link = image.link;
            block.indent = indent as i32;
            block.center = center;
            blocks.push(block);
        }
        _ => {
            let mut block = ParsedBlock::new(61);
            block.images = std::mem::take(images);
            block.indent = indent as i32;
            block.center = center;
            blocks.push(block);
        }
    }
}

fn inline_content<'a>(node: &'a AstNode<'a>) -> (String, String) {
    pieces_to_text(&inline_pieces(node, None))
}

fn pieces_to_text(pieces: &[InlinePiece]) -> (String, String) {
    let mut rich = String::new();
    let mut plain = String::new();

    for piece in pieces {
        match piece {
            InlinePiece::Text(run) => {
                rich.push_str(&run.markdown);
                plain.push_str(&run.plain);
            }
            InlinePiece::Image { alt, .. } => {
                if !plain.is_empty() && !alt.is_empty() {
                    plain.push(' ');
                }
                plain.push_str(alt);
            }
        }
    }

    (rich.trim().to_string(), plain.trim().to_string())
}

fn inline_pieces<'a>(node: &'a AstNode<'a>, active_link: Option<&str>) -> Vec<InlinePiece> {
    let mut pieces = Vec::new();
    // Link opened by a raw `<a href="...">` tag, closed by `</a>`.
    let mut html_link: Option<String> = None;

    for child in node.children() {
        let data = child.data.borrow();
        let start = pieces.len();

        match &data.value {
            NodeValue::Text(text) => {
                pieces.push(InlinePiece::Text(InlineRun {
                    markdown: escape_markdown_text(text),
                    plain: text.to_string(),
                }));
            }

            NodeValue::Code(code) => {
                pieces.push(InlinePiece::Text(InlineRun {
                    markdown: markdown_code(&code.literal),
                    plain: code.literal.clone(),
                }));
            }

            NodeValue::SoftBreak | NodeValue::LineBreak => {
                pieces.push(InlinePiece::Text(InlineRun {
                    markdown: "\n".to_string(),
                    plain: " ".to_string(),
                }));
            }

            NodeValue::Emph => {
                wrap_runs(inline_pieces(child, active_link), "*", "*", &mut pieces);
            }

            NodeValue::Strong => {
                wrap_runs(inline_pieces(child, active_link), "**", "**", &mut pieces);
            }

            NodeValue::Strikethrough => {
                wrap_runs(inline_pieces(child, active_link), "~~", "~~", &mut pieces);
            }

            NodeValue::Link(link) => {
                let destination = escape_link_destination(&link.url);
                for piece in inline_pieces(child, Some(&link.url)) {
                    match piece {
                        InlinePiece::Text(run) => {
                            pieces.push(InlinePiece::Text(InlineRun {
                                markdown: format!("[{}]({})", run.markdown, destination),
                                plain: run.plain,
                            }));
                        }
                        InlinePiece::Image {
                            src, alt, width, ..
                        } => {
                            pieces.push(InlinePiece::Image {
                                src,
                                alt,
                                link: link.url.clone(),
                                width,
                            });
                        }
                    }
                }
            }

            NodeValue::Image(image) => {
                let alt = plain_text_children(child);
                pieces.push(InlinePiece::Image {
                    src: image.url.clone(),
                    alt,
                    link: active_link.unwrap_or("").to_string(),
                    width: 0,
                });
            }

            NodeValue::HtmlInline(raw) => {
                if let Some(tag) = parse_html_tag(raw) {
                    match tag.name.as_str() {
                        "img" if !tag.closing => {
                            if let Some(src) = tag.attr("src") {
                                pieces.push(InlinePiece::Image {
                                    src: src.to_string(),
                                    alt: tag.attr("alt").unwrap_or("").to_string(),
                                    link: active_link.unwrap_or("").to_string(),
                                    width: parse_dimension(tag.attr("width")),
                                });
                            }
                        }
                        "a" => {
                            html_link = if tag.closing {
                                None
                            } else {
                                tag.attr("href")
                                    .filter(|href| !href.is_empty())
                                    .map(str::to_string)
                            };
                        }
                        "br" => {
                            pieces.push(InlinePiece::Text(InlineRun {
                                markdown: "\n".to_string(),
                                plain: " ".to_string(),
                            }));
                        }
                        _ => {}
                    }
                }
            }

            _ => {
                pieces.extend(inline_pieces(child, active_link));
            }
        }

        if let Some(href) = html_link.as_deref() {
            for piece in pieces[start..].iter_mut() {
                attach_link(piece, href);
            }
        }
    }

    pieces
}

/// Apply a link from a raw `<a href>` wrapper to a piece.
fn attach_link(piece: &mut InlinePiece, href: &str) {
    match piece {
        InlinePiece::Text(run) => {
            if !run.plain.trim().is_empty() {
                run.markdown = format!("[{}]({})", run.markdown, escape_link_destination(href));
            }
        }
        InlinePiece::Image { link, .. } => {
            if link.is_empty() {
                *link = href.to_string();
            }
        }
    }
}

fn wrap_runs(inner: Vec<InlinePiece>, prefix: &str, suffix: &str, output: &mut Vec<InlinePiece>) {
    for piece in inner {
        match piece {
            InlinePiece::Text(run) => output.push(InlinePiece::Text(InlineRun {
                markdown: format!("{}{}{}", prefix, run.markdown, suffix),
                plain: run.plain,
            })),
            image => output.push(image),
        }
    }
}

fn plain_text_children<'a>(node: &'a AstNode<'a>) -> String {
    let mut plain = String::new();

    for piece in inline_pieces(node, None) {
        match piece {
            InlinePiece::Text(run) => plain.push_str(&run.plain),
            InlinePiece::Image { alt, .. } => plain.push_str(&alt),
        }
    }

    plain
}

// ───────────────────────── Raw HTML support ─────────────────────────

#[derive(Debug, Clone)]
struct HtmlTag {
    name: String,
    closing: bool,
    attrs: Vec<(String, String)>,
}

impl HtmlTag {
    fn attr(&self, key: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    }
}

#[derive(Debug)]
enum HtmlToken {
    Text(String),
    Tag(HtmlTag),
}

/// Parse a complete tag such as `<img src="x" width="80">` or `</a>`.
fn parse_html_tag(raw: &str) -> Option<HtmlTag> {
    let inner = raw.trim().strip_prefix('<')?.strip_suffix('>')?;
    parse_tag_inner(inner)
}

/// Parse the text between `<` and `>`.
fn parse_tag_inner(inner: &str) -> Option<HtmlTag> {
    let mut inner = inner.trim();
    if inner.starts_with('!') || inner.starts_with('?') {
        return None;
    }

    let closing = inner.starts_with('/');
    if closing {
        inner = inner[1..].trim_start();
    }

    let inner = inner.trim_end_matches('/').trim_end();
    let name_end = inner
        .find(|c: char| c.is_whitespace())
        .unwrap_or(inner.len());
    let name = inner[..name_end].to_ascii_lowercase();

    if !name
        .chars()
        .next()
        .map_or(false, |c| c.is_ascii_alphabetic())
    {
        return None;
    }

    let attrs = if closing {
        Vec::new()
    } else {
        parse_attrs(&inner[name_end..])
    };

    Some(HtmlTag {
        name,
        closing,
        attrs,
    })
}

fn parse_attrs(source: &str) -> Vec<(String, String)> {
    let chars: Vec<char> = source.chars().collect();
    let mut attrs = Vec::new();
    let mut i = 0;

    while i < chars.len() {
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }

        let key_start = i;
        while i < chars.len() && !chars[i].is_whitespace() && chars[i] != '=' {
            i += 1;
        }

        if key_start == i {
            // Stray '=' or end of input; always make progress.
            i += 1;
            continue;
        }

        let key: String = chars[key_start..i]
            .iter()
            .collect::<String>()
            .to_ascii_lowercase();

        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }

        let mut value = String::new();
        if i < chars.len() && chars[i] == '=' {
            i += 1;
            while i < chars.len() && chars[i].is_whitespace() {
                i += 1;
            }

            if i < chars.len() && (chars[i] == '"' || chars[i] == '\'') {
                let quote = chars[i];
                i += 1;
                let value_start = i;
                while i < chars.len() && chars[i] != quote {
                    i += 1;
                }
                value = chars[value_start..i].iter().collect();
                i += 1; // closing quote
            } else {
                let value_start = i;
                while i < chars.len() && !chars[i].is_whitespace() {
                    i += 1;
                }
                value = chars[value_start..i].iter().collect();
            }
        }

        attrs.push((key, decode_entities(&value)));
    }

    attrs
}

/// Index of the closing `>` of the tag at the start of `s`, ignoring `>` in quotes.
fn find_tag_end(s: &str) -> Option<usize> {
    let mut quote: Option<char> = None;

    for (i, c) in s.char_indices().skip(1) {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                }
            }
            None => match c {
                '"' | '\'' => quote = Some(c),
                '>' => return Some(i),
                _ => {}
            },
        }
    }

    None
}

fn tokenize_html(html: &str) -> Vec<HtmlToken> {
    let mut tokens = Vec::new();
    let mut rest = html;

    while !rest.is_empty() {
        let Some(pos) = rest.find('<') else {
            tokens.push(HtmlToken::Text(rest.to_string()));
            break;
        };

        if pos > 0 {
            tokens.push(HtmlToken::Text(rest[..pos].to_string()));
        }
        rest = &rest[pos..];

        if rest.starts_with("<!--") {
            match rest.find("-->") {
                Some(end) => {
                    rest = &rest[end + 3..];
                    continue;
                }
                None => break,
            }
        }

        // A '<' that does not start a tag ("a < b") is just text.
        let starts_tag = rest[1..]
            .chars()
            .next()
            .map_or(false, |c| c.is_ascii_alphabetic() || c == '/' || c == '!');
        if !starts_tag {
            tokens.push(HtmlToken::Text("<".to_string()));
            rest = &rest[1..];
            continue;
        }

        match find_tag_end(rest) {
            Some(end) => {
                if let Some(tag) = parse_tag_inner(&rest[1..end]) {
                    tokens.push(HtmlToken::Tag(tag));
                }
                rest = &rest[end + 1..];
            }
            None => {
                tokens.push(HtmlToken::Text(rest.to_string()));
                break;
            }
        }
    }

    tokens
}

fn decode_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_string();
    }

    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
}

/// `width="120"`, `width="120px"` -> 120. Percentages and junk -> 0.
fn parse_dimension(value: Option<&str>) -> i32 {
    let Some(value) = value else {
        return 0;
    };
    let value = value.trim().trim_end_matches("px").trim();
    value
        .parse::<f32>()
        .ok()
        .filter(|v| *v > 0.0)
        .map(|v| v.round() as i32)
        .unwrap_or(0)
}

fn is_centered(tag: &HtmlTag) -> bool {
    tag.name == "center"
        || tag
            .attr("align")
            .map_or(false, |a| a.trim().eq_ignore_ascii_case("center"))
        || tag.attr("style").map_or(false, |style| {
            style
                .to_ascii_lowercase()
                .replace(' ', "")
                .contains("text-align:center")
        })
}

fn heading_level(name: &str) -> Option<i32> {
    match name {
        "h1" => Some(1),
        "h2" => Some(2),
        "h3" => Some(3),
        "h4" => Some(4),
        "h5" => Some(5),
        "h6" => Some(6),
        _ => None,
    }
}

fn is_html_container(name: &str) -> bool {
    matches!(
        name,
        "p" | "div"
            | "center"
            | "section"
            | "header"
            | "footer"
            | "blockquote"
            | "details"
            | "summary"
            | "ul"
            | "ol"
            | "li"
            | "table"
            | "thead"
            | "tbody"
            | "tr"
            | "td"
            | "th"
    ) || heading_level(name).is_some()
}

struct HtmlContext {
    name: String,
    centered: bool,
    heading: Option<i32>,
}

fn flush_html(
    pieces: &mut Vec<InlinePiece>,
    stack: &[HtmlContext],
    text_type: i32,
    blocks: &mut Vec<ParsedBlock>,
) {
    if pieces.is_empty() {
        return;
    }

    let taken = std::mem::take(pieces);
    let centered = stack.last().map_or(false, |ctx| ctx.centered);
    let block_type = stack
        .iter()
        .rev()
        .find_map(|ctx| ctx.heading)
        .unwrap_or(text_type);

    emit_pieces(taken, block_type, "", 0, centered, blocks);
}

/// Best-effort conversion of an HTML block into native blocks.
///
/// Supports `<img>`, `<a href>` wrappers (including around images), `<p>`/`<div>`/
/// `<center>` with `align="center"`, `<h1>`-`<h6>`, `<br>`, `<hr>` and plain text.
/// Anything else is ignored but its text content is kept.
fn html_to_blocks(html: &str, blocks: &mut Vec<ParsedBlock>, text_type: i32) {
    let mut pieces: Vec<InlinePiece> = Vec::new();
    let mut stack: Vec<HtmlContext> = Vec::new();
    let mut link: Option<String> = None;
    let mut skip: Option<String> = None;

    for token in tokenize_html(html) {
        match token {
            HtmlToken::Text(text) => {
                if skip.is_some() {
                    continue;
                }

                let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
                let text = if collapsed.is_empty() {
                    if text.is_empty() {
                        continue;
                    }
                    " ".to_string()
                } else {
                    let mut value = String::new();
                    if text.starts_with(char::is_whitespace) {
                        value.push(' ');
                    }
                    value.push_str(&collapsed);
                    if text.ends_with(char::is_whitespace) {
                        value.push(' ');
                    }
                    value
                };

                let text = decode_entities(&text);
                if pieces.is_empty() && text.trim().is_empty() {
                    continue;
                }

                let mut piece = InlinePiece::Text(InlineRun {
                    markdown: escape_markdown_text(&text),
                    plain: text,
                });
                if let Some(href) = link.as_deref() {
                    attach_link(&mut piece, href);
                }
                pieces.push(piece);
            }

            HtmlToken::Tag(tag) => {
                let name = tag.name.as_str();

                if name == "script" || name == "style" {
                    if !tag.closing {
                        skip = Some(tag.name.clone());
                    } else if skip.as_deref() == Some(name) {
                        skip = None;
                    }
                    continue;
                }
                if skip.is_some() {
                    continue;
                }

                match name {
                    "img" if !tag.closing => {
                        if let Some(src) = tag.attr("src") {
                            pieces.push(InlinePiece::Image {
                                src: src.to_string(),
                                alt: tag.attr("alt").unwrap_or("").to_string(),
                                link: link.clone().unwrap_or_default(),
                                width: parse_dimension(tag.attr("width")),
                            });
                        }
                    }

                    "a" => {
                        link = if tag.closing {
                            None
                        } else {
                            tag.attr("href")
                                .filter(|href| !href.is_empty())
                                .map(str::to_string)
                        };
                    }

                    "br" => {
                        pieces.push(InlinePiece::Text(InlineRun {
                            markdown: "\n".to_string(),
                            plain: " ".to_string(),
                        }));
                    }

                    "hr" => {
                        flush_html(&mut pieces, &stack, text_type, blocks);
                        blocks.push(ParsedBlock::new(50));
                    }

                    _ if is_html_container(name) => {
                        flush_html(&mut pieces, &stack, text_type, blocks);

                        if tag.closing {
                            if let Some(index) = stack.iter().rposition(|ctx| ctx.name == tag.name)
                            {
                                stack.truncate(index);
                            }
                        } else {
                            let parent_centered = stack.last().map_or(false, |ctx| ctx.centered);
                            stack.push(HtmlContext {
                                name: tag.name.clone(),
                                centered: parent_centered || is_centered(&tag),
                                heading: heading_level(name),
                            });
                        }
                    }

                    _ => {}
                }
            }
        }
    }

    flush_html(&mut pieces, &stack, text_type, blocks);
}

// ───────────────────────── Markdown helpers ─────────────────────────

fn escape_markdown_text(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());

    for character in text.chars() {
        match character {
            '\\' | '`' | '*' | '_' | '[' | ']' | '~' => {
                escaped.push('\\');
                escaped.push(character);
            }
            _ => escaped.push(character),
        }
    }

    escaped
}

fn markdown_code(text: &str) -> String {
    let fence = if text.contains("```") {
        "````"
    } else if text.contains("``") {
        "```"
    } else if text.contains('`') {
        "``"
    } else {
        "`"
    };

    if fence.len() > 1 {
        format!("{} {} {}", fence, text, fence)
    } else {
        format!("{}{}{}", fence, text, fence)
    }
}

fn escape_link_destination(url: &str) -> String {
    let mut escaped = String::with_capacity(url.len());

    for character in url.chars() {
        match character {
            '\\' => escaped.push_str("%5C"),
            '(' => escaped.push_str("%28"),
            ')' => escaped.push_str("%29"),
            ' ' => escaped.push_str("%20"),
            _ => escaped.push(character),
        }
    }

    escaped
}

fn slugify(text: &str) -> String {
    let mut slug = String::new();
    let mut last_was_dash = false;

    for character in text.chars().flat_map(|c| c.to_lowercase()) {
        if character.is_alphanumeric() {
            slug.push(character);
            last_was_dash = false;
        } else if (character.is_whitespace() || character == '-' || character == '_')
            && !slug.is_empty()
            && !last_was_dash
        {
            slug.push('-');
            last_was_dash = true;
        }
    }

    while slug.ends_with('-') {
        slug.pop();
    }

    if slug.is_empty() {
        "section".to_string()
    } else {
        slug
    }
}
