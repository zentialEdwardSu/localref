//! FormulaNet preprocessing, decoding, tokenization, and quality filtering.

use crate::*;

pub(crate) fn recognize_formula(
    session: &mut Session,
    crop: &RgbImage,
) -> Result<Vec<i64>, String> {
    let cropped = crop_formula_margin(crop);
    // Match PaddleX/Pillow: resize the short edge to 384 with bilinear, then
    // thumbnail the long edge to 384 with bicubic. The trained decoder is
    // sensitive to the antialiasing produced by this apparently redundant
    // two-stage resize.
    let (first_width, first_height) = if cropped.width() <= cropped.height() {
        (
            384,
            (384_u64 * cropped.height() as u64 / cropped.width().max(1) as u64)
                .max(1) as u32,
        )
    } else {
        (
            (384_u64 * cropped.width() as u64 / cropped.height().max(1) as u64)
                .max(1) as u32,
            384,
        )
    };
    let first =
        resize(&cropped, first_width, first_height, FilterType::Triangle);
    let scale = 384.0 / first.width().max(first.height()).max(1) as f32;
    let width = ((first.width() as f32 * scale).round() as u32).clamp(1, 384);
    let height =
        ((first.height() as f32 * scale).round() as u32).clamp(1, 384);
    let resized = resize(&first, width, height, FilterType::CatmullRom);
    let offset_x = (384 - width) / 2;
    let offset_y = (384 - height) / 2;
    // PaddleX's ImageOps.expand call uses its default black fill.
    let mut canvas = RgbImage::from_pixel(384, 384, Rgb([0, 0, 0]));
    image::imageops::replace(
        &mut canvas,
        &resized,
        offset_x.into(),
        offset_y.into(),
    );
    let mut tensor = vec![0.0_f32; 384 * 384];
    for (x, y, pixel) in canvas.enumerate_pixels() {
        // The official transform receives an RGB array but calls BGR2GRAY.
        let gray = 0.114 * pixel[0] as f32
            + 0.587 * pixel[1] as f32
            + 0.299 * pixel[2] as f32;
        tensor[y as usize * 384 + x as usize] =
            (gray / 255.0 - 0.7931) / 0.1738;
    }
    let input = Tensor::<f32>::from_array(([1usize, 1, 384, 384], tensor))
        .map_err(|e| e.to_string())?;
    let output = session
        .run(ort::inputs!["x" => input])
        .map_err(|e| format!("PP-FormulaNet-S inference failed: {e}"))?;
    let (_, ids) = output[0]
        .try_extract_tensor::<i64>()
        .map_err(|e| format!("read PP-FormulaNet-S tokens: {e}"))?;
    Ok(ids.to_vec())
}

pub(crate) fn crop_formula_margin(image: &RgbImage) -> RgbImage {
    let mut min_luma = u8::MAX;
    let mut max_luma = u8::MIN;
    let mut lumas =
        Vec::with_capacity((image.width() * image.height()) as usize);
    for pixel in image.pixels() {
        let luma = (0.299 * pixel[0] as f32
            + 0.587 * pixel[1] as f32
            + 0.114 * pixel[2] as f32)
            .round() as u8;
        min_luma = min_luma.min(luma);
        max_luma = max_luma.max(luma);
        lumas.push(luma);
    }
    if min_luma == max_luma {
        return image.clone();
    }
    let range = (max_luma - min_luma) as f32;
    let mut bounds = [image.width(), image.height(), 0, 0];
    for (index, luma) in lumas.into_iter().enumerate() {
        let normalized = (luma - min_luma) as f32 / range * 255.0;
        if normalized < 200.0 {
            let x = index as u32 % image.width();
            let y = index as u32 / image.width();
            bounds[0] = bounds[0].min(x);
            bounds[1] = bounds[1].min(y);
            bounds[2] = bounds[2].max(x + 1);
            bounds[3] = bounds[3].max(y + 1);
        }
    }
    if bounds[2] <= bounds[0] || bounds[3] <= bounds[1] {
        image.clone()
    } else {
        crop_imm(
            image,
            bounds[0],
            bounds[1],
            bounds[2] - bounds[0],
            bounds[3] - bounds[1],
        )
        .to_image()
    }
}

pub(crate) fn pad_formula_bbox(
    [x1, y1, x2, y2]: [u32; 4],
    width: u32,
    height: u32,
) -> [u32; 4] {
    let box_height = y2.saturating_sub(y1).max(1);
    // DocLayout already predicts the full formula box. A small guard band is
    // enough for accents and antialiasing; padding by a full box height pulls
    // neighboring prose into FormulaNet on dense two-column papers.
    let horizontal = (box_height / 4).clamp(2, 24);
    let vertical = (box_height / 8).clamp(1, 12);
    [
        x1.saturating_sub(horizontal),
        y1.saturating_sub(vertical),
        x2.saturating_add(horizontal).min(width),
        y2.saturating_add(vertical).min(height),
    ]
}

pub(crate) fn implausible_formula_reason(latex: &str) -> Option<&'static str> {
    let text = latex.trim();
    if text.is_empty() {
        return Some("empty");
    }
    if text.len() > 4_096 {
        return Some("too_long");
    }
    if text.contains("[UNK]") {
        return Some("unknown_token");
    }
    if text.contains("\\begin array") || text.contains("\\end array") {
        return Some("malformed_environment");
    }
    if text.contains("oversetoverset") {
        return Some("repeated_token");
    }
    if text.contains("!!!") || text.contains("???") {
        return Some("repeated_punctuation");
    }
    if text.starts_with("\\_") {
        return Some("malformed_math_prefix");
    }
    if has_bare_latex_keyword(text) {
        return Some("missing_command_escape");
    }
    if has_unsupported_latex_command(text) {
        return Some("unsupported_command");
    }
    if !balanced_formula_braces(text) {
        return Some("unbalanced_braces");
    }
    if !balanced_formula_environments(text) {
        return Some("unbalanced_environment");
    }
    if text.matches("\\quad").count() > 12 || text.matches("\\ ").count() > 24
    {
        return Some("excessive_spacing");
    }
    if longest_spaced_letter_run(text) >= 7 {
        return Some("prose_like_output");
    }
    if !text.chars().any(|character| {
        character.is_alphanumeric() || "\\{}_^=+-".contains(character)
    }) {
        return Some("no_formula_content");
    }
    None
}

pub(crate) fn has_bare_latex_keyword(text: &str) -> bool {
    const KEYWORDS: &[&str] = &[
        "bar",
        "begin",
        "bf",
        "boldsymbol",
        "cal",
        "displaystyle",
        "dot",
        "end",
        "frac",
        "hat",
        "mathbb",
        "mathcal",
        "mathbf",
        "mathfrak",
        "mathrm",
        "mathsf",
        "mathtt",
        "operatorname",
        "overbrace",
        "scriptstyle",
        "sf",
        "textstyle",
        "tilde",
        "tt",
        "underbrace",
        "underset",
    ];
    KEYWORDS.iter().any(|keyword| {
        text.match_indices(keyword).any(|(index, _)| {
            let bytes = text.as_bytes();
            let before_is_letter =
                index > 0 && bytes[index - 1].is_ascii_alphabetic();
            let after = index + keyword.len();
            let after_is_letter =
                after < bytes.len() && bytes[after].is_ascii_alphabetic();
            !before_is_letter
                && !after_is_letter
                && (index == 0 || !is_escaped(bytes, index))
        })
    })
}

pub(crate) fn has_unsupported_latex_command(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'\\' {
            index += 1;
            continue;
        }
        if bytes.get(index + 1) == Some(&b'\\') {
            index += 2;
            continue;
        }
        let start = index + 1;
        let mut end = start;
        while end < bytes.len() && bytes[end].is_ascii_alphabetic() {
            end += 1;
        }
        if end > start {
            let command = &text[start..end];
            if !supported_latex_command(command) {
                return true;
            }
            index = end;
        } else {
            index += 1;
        }
    }
    false
}

pub(crate) fn supported_latex_command(command: &str) -> bool {
    matches!(
        command,
        "Delta"
            | "Big"
            | "Bigg"
            | "Biggl"
            | "Biggr"
            | "Bigl"
            | "Bigr"
            | "Gamma"
            | "Lambda"
            | "Omega"
            | "Phi"
            | "Pi"
            | "Psi"
            | "Theta"
            | "Xi"
            | "alpha"
            | "argmax"
            | "argmin"
            | "approx"
            | "array"
            | "b"
            | "bar"
            | "begin"
            | "bf"
            | "boldmath"
            | "boldsymbol"
            | "bot"
            | "bullet"
            | "cases"
            | "cal"
            | "cdot"
            | "cdots"
            | "centerdot"
            | "chi"
            | "circ"
            | "cos"
            | "det"
            | "diag"
            | "delta"
            | "displaystyle"
            | "dot"
            | "dots"
            | "ell"
            | "end"
            | "epsilon"
            | "exists"
            | "exp"
            | "forall"
            | "frac"
            | "gamma"
            | "ge"
            | "geq"
            | "hat"
            | "hbox"
            | "in"
            | "infty"
            | "int"
            | "iota"
            | "it"
            | "jmath"
            | "l"
            | "langle"
            | "ldots"
            | "le"
            | "left"
            | "leftarrow"
            | "leftrightarrow"
            | "leq"
            | "lim"
            | "ll"
            | "ln"
            | "log"
            | "mathbb"
            | "mathcal"
            | "mathbf"
            | "mathfrak"
            | "mathit"
            | "mathop"
            | "mathscr"
            | "mathrm"
            | "mathsf"
            | "mathtt"
            | "mid"
            | "min"
            | "max"
            | "mp"
            | "mu"
            | "nabla"
            | "neq"
            | "normalfont"
            | "nu"
            | "odot"
            | "omega"
            | "operatorname"
            | "over"
            | "overbrace"
            | "overline"
            | "overset"
            | "parallel"
            | "partial"
            | "perp"
            | "phantom"
            | "phi"
            | "pi"
            | "pm"
            | "prime"
            | "prod"
            | "Pr"
            | "propto"
            | "psi"
            | "qquad"
            | "quad"
            | "r"
            | "rangle"
            | "Re"
            | "right"
            | "rightarrow"
            | "rm"
            | "scriptstyle"
            | "scriptsize"
            | "sf"
            | "sim"
            | "sin"
            | "stackrel"
            | "star"
            | "substack"
            | "sum"
            | "tau"
            | "textbf"
            | "text"
            | "textit"
            | "textsf"
            | "textstyle"
            | "texttt"
            | "theta"
            | "tilde"
            | "times"
            | "tiny"
            | "top"
            | "triangleq"
            | "tt"
            | "underbrace"
            | "underline"
            | "uparrow"
            | "downarrow"
            | "underrightarrow"
            | "underset"
            | "varphi"
            | "varrho"
            | "vartheta"
            | "vec"
            | "vert"
            | "Vert"
            | "widehat"
            | "widetilde"
            | "zeta"
    )
}

pub(crate) fn balanced_formula_braces(text: &str) -> bool {
    let chars = text.as_bytes();
    let mut depth = 0_i32;
    for (index, &character) in chars.iter().enumerate() {
        if !matches!(character, b'{' | b'}') || is_escaped(chars, index) {
            continue;
        }
        if character == b'{' {
            depth += 1;
        } else {
            depth -= 1;
            if depth < 0 {
                return false;
            }
        }
    }
    depth == 0
}

pub(crate) fn is_escaped(text: &[u8], index: usize) -> bool {
    let mut slashes = 0;
    let mut cursor = index;
    while cursor > 0 && text[cursor - 1] == b'\\' {
        slashes += 1;
        cursor -= 1;
    }
    slashes % 2 == 1
}

pub(crate) fn balanced_formula_environments(text: &str) -> bool {
    let mut stack = Vec::<&str>::new();
    let mut cursor = 0;
    loop {
        let remainder = &text[cursor..];
        let next_begin = remainder.find("\\begin{");
        let next_end = remainder.find("\\end{");
        let Some((relative, is_begin)) = (match (next_begin, next_end) {
            (Some(begin), Some(end)) => {
                Some(if begin < end { (begin, true) } else { (end, false) })
            }
            (Some(begin), None) => Some((begin, true)),
            (None, Some(end)) => Some((end, false)),
            (None, None) => None,
        }) else {
            break;
        };
        let start = cursor + relative;
        let name_start = start + if is_begin { 7 } else { 5 };
        let Some(name_end_relative) = text[name_start..].find('}') else {
            return false;
        };
        let name_end = name_start + name_end_relative;
        let name = &text[name_start..name_end];
        if is_begin {
            stack.push(name);
        } else if stack.pop() != Some(name) {
            return false;
        }
        cursor = name_end + 1;
    }
    stack.is_empty()
}

pub(crate) fn longest_spaced_letter_run(text: &str) -> usize {
    let chars = text.as_bytes();
    let mut longest = 0;
    let mut run = 0;
    let mut index = 0;
    while index < chars.len() {
        if !chars[index].is_ascii_alphabetic() {
            run = 0;
            index += 1;
            continue;
        }
        let mut next = index + 1;
        let mut separator = false;
        while next < chars.len()
            && (chars[next].is_ascii_whitespace() || chars[next] == b'~')
        {
            separator = true;
            next += 1;
        }
        if separator && next < chars.len() && chars[next].is_ascii_alphabetic()
        {
            run = if run == 0 { 2 } else { run + 1 };
            longest = longest.max(run);
        } else {
            run = 0;
        }
        index = next.max(index + 1);
    }
    longest
}

pub(crate) struct FormulaTokenizer {
    tokens: Vec<Option<String>>,
    special: Vec<bool>,
    byte_decoder: BTreeMap<char, u8>,
}

impl FormulaTokenizer {
    pub(crate) fn load(path: &Path) -> Result<Self, String> {
        let config: Value = read_json(path)?;
        let tokenizer = config
            .pointer("/PostProcess/character_dict/fast_tokenizer_file")
            .ok_or("formula config contains no tokenizer")?;
        let vocab = tokenizer
            .pointer("/model/vocab")
            .and_then(Value::as_object)
            .ok_or("formula tokenizer contains no vocabulary")?;
        let max_id = vocab
            .values()
            .filter_map(Value::as_u64)
            .max()
            .ok_or("formula tokenizer vocabulary is empty")?
            as usize;
        let mut tokens = vec![None; max_id + 1];
        for (token, id) in vocab {
            if let Some(id) = id.as_u64().map(|id| id as usize)
                && id < tokens.len()
            {
                tokens[id] = Some(token.clone());
            }
        }
        let mut special = vec![false; tokens.len()];
        if let Some(added) =
            tokenizer.get("added_tokens").and_then(Value::as_array)
        {
            for token in added {
                if token.get("special").and_then(Value::as_bool) == Some(true)
                    && let Some(id) = token.get("id").and_then(Value::as_u64)
                    && let Some(slot) = special.get_mut(id as usize)
                {
                    *slot = true;
                }
            }
        }
        Ok(Self { tokens, special, byte_decoder: byte_level_decoder() })
    }

    pub(crate) fn decode(&self, ids: &[i64]) -> Result<String, String> {
        let mut encoded = String::new();
        for &id in ids {
            if id == 2 {
                break;
            }
            let Ok(index) = usize::try_from(id) else { continue };
            if self.special.get(index).copied().unwrap_or(false) {
                continue;
            }
            if let Some(Some(token)) = self.tokens.get(index) {
                encoded.push_str(token);
            }
        }
        let mut bytes = Vec::with_capacity(encoded.len());
        for character in encoded.chars() {
            if let Some(byte) = self.byte_decoder.get(&character) {
                bytes.push(*byte);
            } else {
                let mut buffer = [0_u8; 4];
                bytes.extend_from_slice(
                    character.encode_utf8(&mut buffer).as_bytes(),
                );
            }
        }
        let decoded = String::from_utf8(bytes)
            .map_err(|e| format!("decode formula tokenizer bytes: {e}"))?;
        Ok(normalize_formula_spacing(&decoded))
    }
}

pub(crate) fn byte_level_decoder() -> BTreeMap<char, u8> {
    let mut bytes = (b'!'..=b'~').collect::<Vec<_>>();
    bytes.extend(0xA1..=0xAC);
    bytes.extend(0xAE..=0xFF);
    let mut codepoints = bytes.iter().map(|&b| b as u32).collect::<Vec<_>>();
    let mut extra = 0_u32;
    for byte in 0_u16..=255 {
        if !bytes.contains(&(byte as u8)) {
            bytes.push(byte as u8);
            codepoints.push(256 + extra);
            extra += 1;
        }
    }
    codepoints.into_iter().filter_map(char::from_u32).zip(bytes).collect()
}

pub(crate) fn normalize_formula_spacing(text: &str) -> String {
    let chars = text.chars().collect::<Vec<_>>();
    let mut output = String::with_capacity(text.len());
    let mut index = 0;
    while index < chars.len() {
        if !chars[index].is_whitespace() {
            output.push(chars[index]);
            index += 1;
            continue;
        }
        let previous = output.chars().next_back();
        while index < chars.len() && chars[index].is_whitespace() {
            index += 1;
        }
        let next = chars.get(index).copied();
        if previous == Some('\\')
            || previous.is_some_and(|c| c.is_ascii_alphabetic())
                && next.is_some_and(|c| c.is_ascii_alphabetic())
        {
            output.push(' ');
        }
    }
    output.trim().trim_matches('"').to_string()
}
