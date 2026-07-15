//! RAG artifact generation, semantic-region images, and Markdown fusion.

use crate::*;

#[derive(Serialize)]
pub(crate) struct Chunk<'a> {
    id: String,
    pub(crate) item_id: &'a str,
    title: &'a str,
    page_hint: usize,
    text: String,
    source: &'static str,
}

pub(crate) fn save_visual_regions(
    page: &RgbImage,
    page_number: usize,
    directory: &Path,
    regions: &mut [LayoutRegion],
) -> Result<(), String> {
    let keep = visual_region_keep_mask(regions);
    for (index, region) in regions.iter_mut().enumerate() {
        if !keep[index] {
            continue;
        }
        let [x1, y1, x2, y2] = region.bbox;
        let crop = crop_imm(page, x1, y1, x2 - x1, y2 - y1).to_image();
        let file =
            format!("page-{page_number:04}-{}-{index:03}.png", region.label);
        crop.save(directory.join(&file))
            .map_err(|e| format!("write semantic region image: {e}"))?;
        region.asset = Some(format!("regions/{file}"));
    }
    Ok(())
}

pub(crate) fn visual_region_keep_mask(regions: &[LayoutRegion]) -> Vec<bool> {
    regions
        .iter()
        .enumerate()
        .map(|(index, region)| {
            if !matches!(region.label.as_str(), "chart" | "image") {
                return false;
            }
            !regions.iter().enumerate().any(|(other_index, other)| {
                other_index != index
                    && matches!(other.label.as_str(), "chart" | "image")
                    && bbox_iou(region.bbox, other.bbox) >= 0.80
                    && (other.label == "chart" && region.label != "chart"
                        || other.label == region.label
                            && (other.score > region.score
                                || other.score == region.score
                                    && other_index < index))
            })
        })
        .collect()
}

#[derive(Debug)]
pub(crate) struct FusedBlock {
    pub(crate) bbox: [u32; 4],
    pub(crate) content: String,
}

pub(crate) fn fuse_layout_markdown(
    pages: &[ParsedPage],
    analyses: &[(usize, PageAnalysis)],
) -> String {
    pages
        .iter()
        .map(|page| {
            analyses
                .iter()
                .find(|(number, _)| *number == page.page_number)
                .map(|(_, analysis)| fuse_page_markdown(page, analysis))
                .filter(|markdown| !markdown.trim().is_empty())
                .unwrap_or_else(|| page.markdown.clone())
                .trim()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n\n-----\n\n")
}

pub(crate) fn fuse_page_markdown(
    page: &ParsedPage,
    analysis: &PageAnalysis,
) -> String {
    let mut assigned = vec![Vec::<&TextItem>::new(); analysis.regions.len()];
    let mut unassigned = Vec::new();
    for item in
        page.text_items.iter().filter(|item| !item.text.trim().is_empty())
    {
        let center_x = (item.x + item.width / 2.0) / page.page_width.max(1.0)
            * analysis.width as f32;
        let center_y = (item.y + item.height / 2.0)
            / page.page_height.max(1.0)
            * analysis.height as f32;
        let region = analysis
            .regions
            .iter()
            .enumerate()
            .filter(|(_, region)| contains(region.bbox, center_x, center_y))
            .max_by(|(_, left), (_, right)| {
                layout_assignment_priority(left)
                    .cmp(&layout_assignment_priority(right))
                    .then_with(|| left.score.total_cmp(&right.score))
            })
            .map(|(index, _)| index);
        if let Some(index) = region {
            assigned[index].push(item);
        } else if center_y < analysis.height as f32 * 0.95 {
            unassigned.push(item);
        }
    }

    let mut blocks = Vec::new();
    for (index, region) in analysis.regions.iter().enumerate() {
        let content = match region.label.as_str() {
            "formula" => analysis
                .formulas
                .iter()
                .filter_map(|formula| {
                    let overlap = bbox_iou(formula.bbox, region.bbox);
                    (overlap >= 0.50).then_some((formula, overlap))
                })
                .max_by(|(_, left), (_, right)| left.total_cmp(right))
                .map(|(formula, _)| {
                    format!("$$\n{}\n$$", formula.latex.trim())
                })
                .or_else(|| format_formula_text_fallback(&assigned[index])),
            "formula_number" => format_formula_text_fallback(&assigned[index]),
            "chart" | "image" => {
                region.asset.as_ref().map(|asset| format!("![]({asset})"))
            }
            label if textual_layout_label(label) => {
                format_region_text(label, &assigned[index])
            }
            _ => None,
        };
        if let Some(content) = content.filter(|value| !value.trim().is_empty())
        {
            blocks.push(FusedBlock { bbox: region.bbox, content });
        }
    }

    // Layout detection is deliberately not allowed to erase native PDF text.
    // Items outside every semantic box become small fallback blocks, while
    // items inside header/footer/chart/formula boxes remain suppressed.
    for item in unassigned {
        let x1 = (item.x / page.page_width.max(1.0) * analysis.width as f32)
            .round()
            .max(0.0) as u32;
        let y1 = (item.y / page.page_height.max(1.0) * analysis.height as f32)
            .round()
            .max(0.0) as u32;
        let x2 = ((item.x + item.width) / page.page_width.max(1.0)
            * analysis.width as f32)
            .round()
            .clamp(0.0, analysis.width as f32) as u32;
        let y2 = ((item.y + item.height) / page.page_height.max(1.0)
            * analysis.height as f32)
            .round()
            .clamp(0.0, analysis.height as f32) as u32;
        blocks.push(FusedBlock {
            bbox: [x1, y1, x2.max(x1 + 1), y2.max(y1 + 1)],
            content: item.text.trim().to_string(),
        });
    }
    merge_adjacent_headings(
        order_fused_blocks(blocks, analysis.width),
        analysis.height,
    )
    .into_iter()
    .map(|block| block.content)
    .collect::<Vec<_>>()
    .join("\n\n")
}

pub(crate) fn contains([x1, y1, x2, y2]: [u32; 4], x: f32, y: f32) -> bool {
    x >= x1 as f32 && x <= x2 as f32 && y >= y1 as f32 && y <= y2 as f32
}

pub(crate) fn layout_assignment_priority(region: &LayoutRegion) -> u8 {
    match region.label.as_str() {
        "formula" | "formula_number" | "chart" | "image" | "header_image"
        | "footer_image" | "header" | "footer" | "seal" => 5,
        "doc_title" | "paragraph_title" | "table" | "reference" => 4,
        "abstract" | "figure_title" | "chart_title" | "table_title" => 3,
        "text" | "content" | "footnote" | "aside_text" => 2,
        _ => 1,
    }
}

pub(crate) fn textual_layout_label(label: &str) -> bool {
    matches!(
        label,
        "doc_title"
            | "paragraph_title"
            | "abstract"
            | "text"
            | "content"
            | "figure_title"
            | "chart_title"
            | "table"
            | "table_title"
            | "reference"
            | "footnote"
            | "aside_text"
            | "algorithm"
    )
}

pub(crate) fn format_region_text(
    label: &str,
    items: &[&TextItem],
) -> Option<String> {
    let lines = text_item_lines(items);
    if lines.is_empty() {
        return None;
    }
    let text = if matches!(label, "table" | "reference" | "algorithm") {
        lines.join("  \n")
    } else {
        join_wrapped_lines(&lines)
    };
    Some(match label {
        "doc_title" => format!("# {text}"),
        "paragraph_title" => format!("## {text}"),
        "figure_title" | "chart_title" | "table_title" => {
            format!("**{text}**")
        }
        "abstract" => format!("**Abstract**\n\n{text}"),
        _ => text,
    })
}

pub(crate) fn format_formula_text_fallback(
    items: &[&TextItem],
) -> Option<String> {
    let lines = text_item_lines(items);
    (!lines.is_empty()).then(|| lines.join("  \n"))
}

pub(crate) fn text_item_lines(items: &[&TextItem]) -> Vec<String> {
    let mut items = items.to_vec();
    items.sort_by(|left, right| {
        left.y.total_cmp(&right.y).then_with(|| left.x.total_cmp(&right.x))
    });
    let mut lines: Vec<(f32, f32, Vec<&TextItem>)> = Vec::new();
    for item in items {
        let tolerance = (item.height * 0.6).max(2.0);
        if let Some((line_y, line_height, line_items)) = lines.last_mut()
            && (item.y - *line_y).abs() <= tolerance.max(*line_height * 0.6)
        {
            *line_height = line_height.max(item.height);
            line_items.push(item);
        } else {
            lines.push((item.y, item.height, vec![item]));
        }
    }
    lines
        .into_iter()
        .map(|(_, _, mut items)| {
            items.sort_by(|left, right| left.x.total_cmp(&right.x));
            let mut line = String::new();
            for item in items {
                append_inline(&mut line, item.text.trim());
            }
            line
        })
        .collect()
}

pub(crate) fn append_inline(line: &mut String, next: &str) {
    let punctuation = ",.;:!?%)]}，。；：！？、";
    if !line.is_empty()
        && !line.ends_with(char::is_whitespace)
        && !next.starts_with(|ch| punctuation.contains(ch))
    {
        line.push(' ');
    }
    line.push_str(next);
}

pub(crate) fn join_wrapped_lines(lines: &[String]) -> String {
    let mut text = String::new();
    for line in lines {
        if text.ends_with('-')
            && line.chars().next().is_some_and(char::is_lowercase)
        {
            text.pop();
        } else if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(line);
    }
    text
}

pub(crate) fn bbox_iou(left: [u32; 4], right: [u32; 4]) -> f32 {
    let width = left[2].min(right[2]).saturating_sub(left[0].max(right[0]));
    let height = left[3].min(right[3]).saturating_sub(left[1].max(right[1]));
    let intersection = width.saturating_mul(height) as f32;
    let left_area = (left[2] - left[0]).saturating_mul(left[3] - left[1]);
    let right_area = (right[2] - right[0]).saturating_mul(right[3] - right[1]);
    let union = (left_area + right_area) as f32 - intersection;
    intersection / union.max(1.0)
}

pub(crate) fn order_fused_blocks(
    mut blocks: Vec<FusedBlock>,
    page_width: u32,
) -> Vec<FusedBlock> {
    let full_width = (page_width as f32 * 0.62) as u32;
    let mut full = Vec::new();
    let mut narrow = Vec::new();
    for block in blocks.drain(..) {
        if block.bbox[2].saturating_sub(block.bbox[0]) >= full_width {
            full.push(block);
        } else {
            narrow.push(block);
        }
    }
    full.sort_by_key(|block| (block.bbox[1], block.bbox[0]));
    let mut ordered = Vec::new();
    for spanning in full {
        let split = spanning.bbox[1];
        let mut before = Vec::new();
        let mut after = Vec::new();
        for block in narrow.drain(..) {
            let center_y = block.bbox[1].saturating_add(block.bbox[3]) / 2;
            if center_y < split {
                before.push(block);
            } else {
                after.push(block);
            }
        }
        ordered.extend(order_columns(before, page_width));
        ordered.push(spanning);
        narrow = after;
    }
    ordered.extend(order_columns(narrow, page_width));
    ordered
}

pub(crate) fn order_columns(
    blocks: Vec<FusedBlock>,
    page_width: u32,
) -> Vec<FusedBlock> {
    let midpoint = page_width / 2;
    let mut left = Vec::new();
    let mut right = Vec::new();
    for block in blocks {
        let center = block.bbox[0].saturating_add(block.bbox[2]) / 2;
        if center < midpoint {
            left.push(block);
        } else {
            right.push(block);
        }
    }
    let by_position = |left: &FusedBlock, right: &FusedBlock| {
        (left.bbox[1], left.bbox[0]).cmp(&(right.bbox[1], right.bbox[0]))
    };
    left.sort_by(by_position);
    right.sort_by(by_position);
    if left.is_empty() || right.is_empty() {
        left.extend(right);
        left.sort_by(by_position);
        left
    } else {
        left.extend(right);
        left
    }
}

pub(crate) fn merge_adjacent_headings(
    blocks: Vec<FusedBlock>,
    page_height: u32,
) -> Vec<FusedBlock> {
    let mut merged: Vec<FusedBlock> = Vec::new();
    for block in blocks {
        let Some(previous) = merged.last_mut() else {
            merged.push(block);
            continue;
        };
        let overlap = previous.bbox[2]
            .min(block.bbox[2])
            .saturating_sub(previous.bbox[0].max(block.bbox[0]));
        let min_width = previous.bbox[2]
            .saturating_sub(previous.bbox[0])
            .min(block.bbox[2].saturating_sub(block.bbox[0]));
        let gap = block.bbox[1].saturating_sub(previous.bbox[3]);
        if previous.content.starts_with("## ")
            && block.content.starts_with("## ")
            && gap <= (page_height / 50).max(10)
            && overlap.saturating_mul(2) >= min_width
        {
            previous.content.push(' ');
            previous.content.push_str(block.content.trim_start_matches("## "));
            previous.bbox[0] = previous.bbox[0].min(block.bbox[0]);
            previous.bbox[1] = previous.bbox[1].min(block.bbox[1]);
            previous.bbox[2] = previous.bbox[2].max(block.bbox[2]);
            previous.bbox[3] = previous.bbox[3].max(block.bbox[3]);
        } else {
            merged.push(block);
        }
    }
    merged
}

pub(crate) fn chunks_from_markdown<'a>(
    markdown: &str,
    item_id: &'a str,
    title: &'a str,
) -> Vec<Chunk<'a>> {
    let mut chunks = Vec::new();
    let mut page = 1;
    let mut current = String::new();
    for line in markdown.lines() {
        if line.trim() == "-----" {
            page += 1;
        }
        for fragment in utf8_fragments(line, CHUNK_BYTES) {
            if current.len() + fragment.len() + 1 > CHUNK_BYTES
                && !current.trim().is_empty()
            {
                let index = chunks.len() + 1;
                chunks.push(Chunk {
                    id: format!("{item_id}:{index}"),
                    item_id,
                    title,
                    page_hint: page,
                    text: current.trim().to_string(),
                    source: "document.md",
                });
                current.clear();
            }
            current.push_str(fragment);
            current.push('\n');
        }
    }
    if !current.trim().is_empty() {
        let index = chunks.len() + 1;
        chunks.push(Chunk {
            id: format!("{item_id}:{index}"),
            item_id,
            title,
            page_hint: page,
            text: current.trim().to_string(),
            source: "document.md",
        });
    }
    chunks
}

pub(crate) fn utf8_fragments(line: &str, max_bytes: usize) -> Vec<&str> {
    if line.len() <= max_bytes {
        return vec![line];
    }
    let mut fragments = Vec::new();
    let mut rest = line;
    while rest.len() > max_bytes {
        let mut end = max_bytes;
        while !rest.is_char_boundary(end) {
            end -= 1;
        }
        fragments.push(&rest[..end]);
        rest = &rest[end..];
    }
    fragments.push(rest);
    fragments
}
pub(crate) fn write_jsonl(
    path: &Path,
    chunks: &[Chunk<'_>],
) -> Result<(), String> {
    let mut body = String::new();
    for chunk in chunks {
        body.push_str(
            &serde_json::to_string(chunk).map_err(|e| e.to_string())?,
        );
        body.push('\n');
    }
    fs::write(path, body).map_err(io("write chunks"))
}
