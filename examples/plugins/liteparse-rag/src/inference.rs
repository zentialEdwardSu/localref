//! ONNX sessions and page-level OCR, layout, and formula inference.

use crate::*;

pub(crate) struct Sessions {
    detector: Session,
    recognizer: Session,
    formula: Session,
    layout: Session,
    provider: &'static str,
    dictionary: Vec<String>,
    formula_tokenizer: FormulaTokenizer,
    layout_labels: Vec<String>,
}
pub(crate) struct PpOcrV6OnnxEngine {
    sessions: Mutex<Sessions>,
}
impl PpOcrV6OnnxEngine {
    pub(crate) fn load(paths: ModelPaths) -> Result<Self, String> {
        let (detector, provider) = create_session(&paths.detector)?;
        let (recognizer, recognizer_provider) =
            create_session(&paths.recognizer)?;
        // DirectML does not reliably execute the dynamic Loop used by the
        // converted autoregressive decoder. Formula candidates are few, so a
        // CPU session is the predictable cross-platform choice.
        let formula = create_cpu_session(&paths.formula)?;
        // PP-DocLayout-L contains GridSample. Keep it on CPU as well so the
        // semantic routing result is identical on machines with and without
        // a DirectML-capable GPU.
        let layout = create_cpu_session(&paths.layout)?;
        let provider =
            if provider == "directml" && recognizer_provider == "directml" {
                "directml"
            } else {
                "cpu"
            };
        Ok(Self {
            sessions: Mutex::new(Sessions {
                detector,
                recognizer,
                formula,
                layout,
                provider,
                dictionary: load_dictionary(&paths.recognizer_config)?,
                formula_tokenizer: FormulaTokenizer::load(
                    &paths.formula_config,
                )?,
                layout_labels: load_layout_labels(&paths.layout_config)?,
            }),
        })
    }
    pub(crate) fn provider(&self) -> &'static str {
        self.sessions.lock().map(|s| s.provider).unwrap_or("cpu")
    }
    fn recognize_sync(
        &self,
        pixels: &[u8],
        width: u32,
        height: u32,
    ) -> Result<Vec<OcrResult>, String> {
        let source = RgbImage::from_raw(width, height, pixels.to_vec())
            .ok_or("LiteParse supplied invalid RGB page bytes")?;
        let mut sessions =
            self.sessions.lock().map_err(|_| "OCR session lock poisoned")?;
        let boxes = detect(&mut sessions.detector, &source)?;
        let mut result = Vec::new();
        let dictionary = sessions.dictionary.clone();
        for [x1, y1, x2, y2] in boxes {
            let crop = crop_imm(
                &source,
                x1,
                y1,
                x2.saturating_sub(x1).max(1),
                y2.saturating_sub(y1).max(1),
            )
            .to_image();
            let (text, confidence) =
                recognize_line(&mut sessions.recognizer, &crop, &dictionary)?;
            if !text.is_empty() {
                result.push(OcrResult {
                    text,
                    bbox: [x1 as f32, y1 as f32, x2 as f32, y2 as f32],
                    confidence,
                    polygon: Some([
                        [x1 as f32, y1 as f32],
                        [x2 as f32, y1 as f32],
                        [x2 as f32, y2 as f32],
                        [x1 as f32, y2 as f32],
                    ]),
                });
            }
        }
        Ok(result)
    }

    pub(crate) fn analyze_page_sync(
        &self,
        source: &RgbImage,
    ) -> Result<PageAnalysis, String> {
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| "layout/formula session lock poisoned")?;
        let labels = sessions.layout_labels.clone();
        let regions = detect_layout(&mut sessions.layout, source, &labels)?;
        let mut formulas = Vec::new();
        let mut formula_rejections = Vec::new();
        for region in regions.iter().filter(|region| region.label == "formula")
        {
            let crop_bbox =
                pad_formula_bbox(region.bbox, source.width(), source.height());
            let [x1, y1, x2, y2] = crop_bbox;
            let crop = crop_imm(
                source,
                x1,
                y1,
                x2.saturating_sub(x1).max(1),
                y2.saturating_sub(y1).max(1),
            )
            .to_image();
            let ids = recognize_formula(&mut sessions.formula, &crop)?;
            let latex = sessions.formula_tokenizer.decode(&ids)?;
            if let Some(reason) = implausible_formula_reason(&latex) {
                formula_rejections.push(FormulaRejection {
                    bbox: region.bbox,
                    layout_score: region.score,
                    reason,
                });
            } else {
                formulas.push(FormulaResult {
                    latex,
                    bbox: region.bbox,
                    layout_score: region.score,
                });
            }
        }
        Ok(PageAnalysis {
            width: source.width(),
            height: source.height(),
            regions,
            formulas,
            formula_rejections,
        })
    }
}
impl OcrEngine for PpOcrV6OnnxEngine {
    fn name(&self) -> &str {
        "pp-ocrv6-medium-onnx"
    }
    fn recognize<'a, 'b: 'a, 'c: 'a>(
        &'a self,
        pixels: &'c [u8],
        width: u32,
        height: u32,
        _: &'b OcrOptions,
    ) -> Pin<
        Box<
            dyn Future<
                    Output = Result<
                        Vec<OcrResult>,
                        Box<dyn std::error::Error + Send + Sync>,
                    >,
                > + Send
                + '_,
        >,
    > {
        Box::pin(async move {
            self.recognize_sync(pixels, width, height)
                .map_err(|e| std::io::Error::other(e).into())
        })
    }
}

pub(crate) fn create_session(
    model: &Path,
) -> Result<(Session, &'static str), String> {
    let dml = || -> Result<Session, String> {
        let mut builder = Session::builder()
            .map_err(|e| e.to_string())?
            .with_execution_providers([ep::DirectML::default()
                .with_performance_preference(
                    PerformancePreference::HighPerformance,
                )
                .build()])
            .map_err(|e| e.to_string())?;
        builder.commit_from_file(model).map_err(|e| e.to_string())
    };
    if let Ok(session) = dml() {
        return Ok((session, "directml"));
    }
    let mut builder = Session::builder().map_err(|e| e.to_string())?;
    builder
        .commit_from_file(model)
        .map(|session| (session, "cpu"))
        .map_err(|e| format!("create ONNX Runtime session: {e}"))
}

pub(crate) fn create_cpu_session(model: &Path) -> Result<Session, String> {
    let mut builder = Session::builder().map_err(|e| e.to_string())?;
    builder
        .commit_from_file(model)
        .map_err(|e| format!("create CPU ONNX Runtime session: {e}"))
}

pub(crate) fn detect(
    session: &mut Session,
    image: &RgbImage,
) -> Result<Vec<[u32; 4]>, String> {
    let (width, height) = resize_shape(image.width(), image.height(), 960);
    let resized = resize(image, width, height, FilterType::Triangle);
    let mut tensor = vec![0.0_f32; 3 * width as usize * height as usize];
    for (x, y, pixel) in resized.enumerate_pixels() {
        for channel in 0..3 {
            let value = pixel[2 - channel] as f32 / 255.0;
            tensor[channel * width as usize * height as usize
                + y as usize * width as usize
                + x as usize] = (value - [0.485, 0.456, 0.406][channel])
                / [0.229, 0.224, 0.225][channel];
        }
    }
    let input = Tensor::<f32>::from_array((
        [1usize, 3, height as usize, width as usize],
        tensor,
    ))
    .map_err(|e| e.to_string())?;
    let output =
        session.run(ort::inputs![input]).map_err(|e| e.to_string())?;
    let (_, scores) =
        output[0].try_extract_tensor::<f32>().map_err(|e| e.to_string())?;
    let map_width = ((scores.len() as f64 * width as f64 / height as f64)
        .sqrt() as usize)
        .max(1);
    if map_width == 0 {
        return Ok(Vec::new());
    }
    let map_height = scores.len() / map_width;
    let boxes = components(scores, map_width, map_height, 0.30);
    Ok(boxes
        .into_iter()
        .map(|[x1, y1, x2, y2]| {
            [
                x1 as u32 * image.width() / map_width as u32,
                y1 as u32 * image.height() / map_height as u32,
                (x2 as u32 * image.width() / map_width as u32)
                    .min(image.width()),
                (y2 as u32 * image.height() / map_height as u32)
                    .min(image.height()),
            ]
        })
        .filter(|b| b[2] > b[0] && b[3] > b[1])
        .collect())
}

pub(crate) fn detect_layout(
    session: &mut Session,
    image: &RgbImage,
    labels: &[String],
) -> Result<Vec<LayoutRegion>, String> {
    const SIZE: u32 = 640;
    let resized = resize(image, SIZE, SIZE, FilterType::CatmullRom);
    let plane = (SIZE * SIZE) as usize;
    let mut pixels = vec![0.0_f32; 3 * plane];
    for (x, y, pixel) in resized.enumerate_pixels() {
        let offset = y as usize * SIZE as usize + x as usize;
        // The Paddle preprocessing pipeline reads with OpenCV, hence BGR.
        pixels[offset] = pixel[2] as f32 / 255.0;
        pixels[plane + offset] = pixel[1] as f32 / 255.0;
        pixels[2 * plane + offset] = pixel[0] as f32 / 255.0;
    }
    let input = Tensor::<f32>::from_array((
        [1usize, 3, SIZE as usize, SIZE as usize],
        pixels,
    ))
    .map_err(|e| e.to_string())?;
    let im_shape = Tensor::<f32>::from_array((
        [1usize, 2usize],
        vec![SIZE as f32, SIZE as f32],
    ))
    .map_err(|e| e.to_string())?;
    let scale_factor = Tensor::<f32>::from_array((
        [1usize, 2usize],
        vec![
            SIZE as f32 / image.height().max(1) as f32,
            SIZE as f32 / image.width().max(1) as f32,
        ],
    ))
    .map_err(|e| e.to_string())?;
    let output = session
        .run(ort::inputs![
            "image" => input,
            "im_shape" => im_shape,
            "scale_factor" => scale_factor,
        ])
        .map_err(|e| format!("PP-DocLayout-L inference failed: {e}"))?;
    let (_, detections) = output[0]
        .try_extract_tensor::<f32>()
        .map_err(|e| format!("read PP-DocLayout-L detections: {e}"))?;
    let mut regions = Vec::new();
    for row in detections.chunks_exact(6) {
        let class_id = row[0].round().max(0.0) as usize;
        let score = row[1];
        if score < 0.5 || class_id >= labels.len() {
            continue;
        }
        let x1 = row[2].round().clamp(0.0, image.width() as f32) as u32;
        let y1 = row[3].round().clamp(0.0, image.height() as f32) as u32;
        let x2 = row[4].round().clamp(0.0, image.width() as f32) as u32;
        let y2 = row[5].round().clamp(0.0, image.height() as f32) as u32;
        if x2 <= x1 || y2 <= y1 {
            continue;
        }
        regions.push(LayoutRegion {
            label: labels[class_id].clone(),
            score,
            bbox: [x1, y1, x2, y2],
            asset: None,
        });
    }
    Ok(regions)
}

pub(crate) fn recognize_line(
    session: &mut Session,
    crop: &RgbImage,
    dictionary: &[String],
) -> Result<(String, f32), String> {
    let height = 48_u32;
    let width = (crop.width() * height / crop.height().max(1)).clamp(16, 320);
    let line = resize(crop, width, height, FilterType::Triangle);
    let mut padded = RgbImage::from_pixel(320, height, Rgb([255, 255, 255]));
    image::imageops::replace(&mut padded, &line, 0, 0);
    let mut tensor = vec![0.0_f32; 3 * 320 * height as usize];
    for (x, y, pixel) in padded.enumerate_pixels() {
        for c in 0..3 {
            tensor
                [c * 320 * height as usize + y as usize * 320 + x as usize] =
                pixel[2 - c] as f32 / 127.5 - 1.0;
        }
    }
    let input = Tensor::<f32>::from_array((
        [1usize, 3, height as usize, 320usize],
        tensor,
    ))
    .map_err(|e| e.to_string())?;
    let output =
        session.run(ort::inputs![input]).map_err(|e| e.to_string())?;
    let (shape, logits) =
        output[0].try_extract_tensor::<f32>().map_err(|e| e.to_string())?;
    let classes = shape
        .last()
        .copied()
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| {
            "recognizer returned an invalid output shape".to_string()
        })?;
    let supports_space = classes == dictionary.len() + 2;
    if classes != dictionary.len() + 1 && !supports_space {
        return Err(format!(
            "recognizer output has {classes} classes but the configured dictionary has {} entries",
            dictionary.len()
        ));
    }
    let mut previous = 0_usize;
    let mut text = String::new();
    let mut confidence = Vec::new();
    for timestep in logits.chunks_exact(classes) {
        let (index, value) = timestep
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap_or((0, &0.0));
        if index != 0 && index != previous {
            if supports_space && index == dictionary.len() + 1 {
                text.push(' ');
            } else {
                text.push_str(&dictionary[index - 1]);
            }
            let max = *value;
            let norm =
                timestep.iter().map(|logit| (*logit - max).exp()).sum::<f32>();
            confidence.push(1.0 / norm);
        }
        previous = index;
    }
    let score = if confidence.is_empty() {
        0.0
    } else {
        confidence.iter().copied().sum::<f32>() / confidence.len() as f32
    };
    Ok((text, score.clamp(0.0, 1.0)))
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct FormulaResult {
    pub(crate) latex: String,
    pub(crate) bbox: [u32; 4],
    pub(crate) layout_score: f32,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct FormulaRejection {
    pub(crate) bbox: [u32; 4],
    pub(crate) layout_score: f32,
    pub(crate) reason: &'static str,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct LayoutRegion {
    pub(crate) label: String,
    pub(crate) score: f32,
    pub(crate) bbox: [u32; 4],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) asset: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct PageAnalysis {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) regions: Vec<LayoutRegion>,
    pub(crate) formulas: Vec<FormulaResult>,
    pub(crate) formula_rejections: Vec<FormulaRejection>,
}

pub(crate) fn resize_shape(
    width: u32,
    height: u32,
    max_side: u32,
) -> (u32, u32) {
    let scale = (max_side as f32 / width.max(height) as f32).min(1.0);
    let w = ((width as f32 * scale / 32.0).ceil() as u32 * 32).max(32);
    let h = ((height as f32 * scale / 32.0).ceil() as u32 * 32).max(32);
    (w, h)
}
pub(crate) fn components(
    scores: &[f32],
    width: usize,
    height: usize,
    threshold: f32,
) -> Vec<[usize; 4]> {
    let mut seen = vec![false; scores.len()];
    let mut boxes = Vec::new();
    for start in 0..scores.len() {
        if seen[start] || scores[start] < threshold {
            continue;
        }
        let mut queue = VecDeque::from([start]);
        seen[start] = true;
        let (mut min_x, mut min_y, mut max_x, mut max_y, mut count) =
            (width, height, 0, 0, 0);
        while let Some(index) = queue.pop_front() {
            let x = index % width;
            let y = index / width;
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
            count += 1;
            for (nx, ny) in [
                (x.wrapping_sub(1), y),
                (x + 1, y),
                (x, y.wrapping_sub(1)),
                (x, y + 1),
            ] {
                if nx < width && ny < height {
                    let next = ny * width + nx;
                    if !seen[next] && scores[next] >= threshold {
                        seen[next] = true;
                        queue.push_back(next);
                    }
                }
            }
        }
        if count >= 16 {
            boxes.push([
                min_x.saturating_sub(1),
                min_y.saturating_sub(1),
                (max_x + 2).min(width),
                (max_y + 2).min(height),
            ]);
        }
    }
    boxes
}

pub(crate) fn load_dictionary(path: &Path) -> Result<Vec<String>, String> {
    let text =
        fs::read_to_string(path).map_err(io("read recognizer config"))?;
    let yaml: serde_yaml::Value = serde_yaml::from_str(&text)
        .map_err(|e| format!("read recognizer dictionary: {e}"))?;
    let mut values = Vec::new();
    collect_dictionary(&yaml, &mut values);
    if values.is_empty() {
        return Err(
            "recognizer configuration contains no character dictionary".into(),
        );
    }
    Ok(values)
}

pub(crate) fn load_layout_labels(path: &Path) -> Result<Vec<String>, String> {
    let text = fs::read_to_string(path).map_err(io("read layout config"))?;
    let yaml: serde_yaml::Value = serde_yaml::from_str(&text)
        .map_err(|e| format!("read layout labels: {e}"))?;
    let labels = yaml
        .get("label_list")
        .and_then(serde_yaml::Value::as_sequence)
        .ok_or("layout configuration contains no label_list")?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| "layout label is not a string".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    if labels.is_empty() {
        return Err("layout configuration contains no labels".into());
    }
    Ok(labels)
}

pub(crate) fn collect_dictionary(
    value: &serde_yaml::Value,
    output: &mut Vec<String>,
) {
    match value {
        serde_yaml::Value::Mapping(map) => {
            for (key, value) in map {
                if key.as_str() == Some("character_dict")
                    && let serde_yaml::Value::Sequence(chars) = value
                {
                    output.extend(
                        chars
                            .iter()
                            .filter_map(|x| x.as_str().map(str::to_owned)),
                    );
                }
                collect_dictionary(value, output);
            }
        }
        serde_yaml::Value::Sequence(values) => {
            for value in values {
                collect_dictionary(value, output);
            }
        }
        _ => {}
    }
}
