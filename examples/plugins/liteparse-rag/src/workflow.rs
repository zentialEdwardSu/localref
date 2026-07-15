//! Plugin actions, queue processing, extraction orchestration, and status rules.

use crate::*;

pub(crate) async fn run_action(
    action: &str,
    ctx: &ActionContext,
) -> RunOutput {
    if action == "download_models" {
        return download_models(ctx).await;
    }
    let ids = match action {
        "process_for_rag" => ctx.selected.clone(),
        "process_active_for_rag" => ctx.active.clone().into_iter().collect(),
        other => return RunOutput::error(format!("unknown action: {other}")),
    };
    if ids.is_empty() {
        feedback(
            &ctx.client,
            "LiteParse RAG extraction",
            "Select at least one PDF item before starting extraction.",
            NotifyKind::Error,
            true,
        )
        .await;
        return RunOutput::error("select at least one PDF item");
    }
    let root = match library_root() {
        Ok(root) => root,
        Err(e) => {
            feedback(
                &ctx.client,
                "LiteParse RAG extraction failed",
                &e,
                NotifyKind::Error,
                true,
            )
            .await;
            return RunOutput::error(e);
        }
    };
    feedback(
        &ctx.client,
        "LiteParse RAG extraction",
        &format!("Preparing extraction for {} selected paper(s)…", ids.len()),
        NotifyKind::Info,
        false,
    )
    .await;
    let summary = process_ids(&ctx.client, &root, &ids).await;
    let level = if summary.failed.is_empty() {
        LogLevel::Info
    } else {
        LogLevel::Warn
    };
    let text = summary.message();
    let _ = ctx.client.log(PLUGIN, level, &text).await;
    feedback(
        &ctx.client,
        if summary.failed.is_empty() {
            "LiteParse RAG extraction complete"
        } else {
            "LiteParse RAG extraction failed"
        },
        &text,
        if summary.failed.is_empty() {
            NotifyKind::Success
        } else {
            NotifyKind::Error
        },
        true,
    )
    .await;
    if summary.handled() == 0 {
        RunOutput::error(text)
    } else {
        RunOutput::ok(text)
    }
}

pub(crate) async fn download_models(ctx: &ActionContext) -> RunOutput {
    let root = match library_root() {
        Ok(root) => root,
        Err(e) => {
            feedback(
                &ctx.client,
                "LiteParse RAG model download failed",
                &e,
                NotifyKind::Error,
                true,
            )
            .await;
            return RunOutput::error(e);
        }
    };
    let proxy = match proxy_from_params(&ctx.params) {
        Ok(proxy) => proxy,
        Err(e) => {
            feedback(
                &ctx.client,
                "LiteParse RAG model download failed",
                &e,
                NotifyKind::Error,
                true,
            )
            .await;
            return RunOutput::error(e);
        }
    };
    let store = ModelStore::new(&root);
    match store.download(&ctx.client, proxy.as_deref()).await {
        Ok(()) => RunOutput::ok("PP-OCRv6 medium models are ready."),
        Err(e) => {
            feedback(
                &ctx.client,
                "LiteParse RAG model download failed",
                &e,
                NotifyKind::Error,
                true,
            )
            .await;
            RunOutput::error(e)
        }
    }
}

pub(crate) fn proxy_from_params(
    params: &std::collections::HashMap<String, String>,
) -> Result<Option<String>, String> {
    let address = params.get("proxy_address").map_or("", |value| value.trim());
    let port = params.get("proxy_port").map_or("", |value| value.trim());
    if address.is_empty() {
        return if port.is_empty() {
            Ok(None)
        } else {
            Err("proxy port requires a proxy address".to_string())
        };
    }
    let address = if address.contains("://") {
        address.to_string()
    } else {
        format!("http://{address}")
    };
    let mut url = reqwest::Url::parse(&address)
        .map_err(|e| format!("invalid proxy address: {e}"))?;
    if url.host_str().is_none() {
        return Err(
            "proxy address must contain a host name or IP address".to_string()
        );
    }
    if !port.is_empty() {
        let port: u16 = port.parse().map_err(|_| {
            "proxy port must be an integer from 1 to 65535".to_string()
        })?;
        url.set_port(Some(port))
            .map_err(|_| "proxy address cannot accept a port".to_string())?;
    }
    Ok(Some(url.into()))
}

pub(crate) async fn enqueue_hook(endpoint: &str, item_id: &str) -> RunOutput {
    let root = match library_root() {
        Ok(root) => root,
        Err(e) => return RunOutput::error(e),
    };
    match Queue::enqueue(&root, item_id) {
        Ok(()) => {
            let client = LocalrefClient::new(endpoint);
            let _ = client
                .set_item_extra(item_id, EXTRA, "status", Some("queued"))
                .await;
            RunOutput::done()
        }
        Err(e) => RunOutput::error(e),
    }
}

pub(crate) async fn process_queue(endpoint: &str) -> RunOutput {
    let root = match library_root() {
        Ok(root) => root,
        Err(e) => return RunOutput::error(e),
    };
    let ids = match Queue::take(&root) {
        Ok(ids) => ids,
        Err(e) => return RunOutput::error(e),
    };
    if ids.is_empty() {
        return RunOutput::done();
    }
    let client = LocalrefClient::new(endpoint);
    feedback(
        &client,
        "LiteParse RAG queue",
        &format!("Processing {} queued paper(s)…", ids.len()),
        NotifyKind::Info,
        false,
    )
    .await;
    let summary = process_ids(&client, &root, &ids).await;
    for id in &summary.retry_ids {
        let _ = Queue::enqueue(&root, id);
    }
    let text = summary.message();
    let level = if summary.failed.is_empty() {
        LogLevel::Info
    } else {
        LogLevel::Warn
    };
    let _ = client.log(PLUGIN, level, &text).await;
    feedback(
        &client,
        if summary.failed.is_empty() {
            "LiteParse RAG queue complete"
        } else {
            "LiteParse RAG queue failed"
        },
        &text,
        if summary.failed.is_empty() {
            NotifyKind::Success
        } else {
            NotifyKind::Error
        },
        true,
    )
    .await;
    RunOutput::ok(text)
}

/// Report progress through the host status channel. Plugin actions also return
/// their terminal message to the page, avoiding desktop-toast integration
/// during a long model download.
pub(crate) async fn feedback(
    client: &LocalrefClient,
    _title: &str,
    message: &str,
    kind: NotifyKind,
    _notify: bool,
) {
    progress(client, message, kind, true).await;
}

pub(crate) async fn progress(
    client: &LocalrefClient,
    message: &str,
    kind: NotifyKind,
    write_log: bool,
) {
    let _ = client.set_status(message, kind).await;
    if write_log {
        let level = if kind == NotifyKind::Error {
            LogLevel::Warn
        } else {
            LogLevel::Info
        };
        let _ = client.log(PLUGIN, level, message).await;
    }
}

pub(crate) fn library_root() -> Result<PathBuf, String> {
    LocalrefConfig::load()
        .map(|config| config.library_root().to_path_buf())
        .map_err(|e| format!("could not load Localref configuration: {e}"))
}

#[derive(Default)]
pub(crate) struct BatchSummary {
    pub(crate) completed: usize,
    pub(crate) unchanged: usize,
    pub(crate) skipped_no_main: usize,
    pub(crate) failed: Vec<String>,
    pub(crate) retry_ids: Vec<String>,
}
impl BatchSummary {
    pub(crate) fn handled(&self) -> usize {
        self.completed + self.unchanged + self.skipped_no_main
    }

    pub(crate) fn message(&self) -> String {
        let mut message = format!(
            "LiteParse RAG: {} processed, {} unchanged, {} skipped (no main file)",
            self.completed, self.unchanged, self.skipped_no_main
        );
        if !self.failed.is_empty() {
            message.push_str(&format!(
                "; {} failed: {}",
                self.failed.len(),
                self.failed.join("; ")
            ));
        }
        message
    }
}

pub(crate) async fn process_ids(
    client: &LocalrefClient,
    root: &Path,
    ids: &[String],
) -> BatchSummary {
    let mut summary = BatchSummary::default();
    let mut eligible = Vec::<(usize, String)>::new();
    for (index, id) in ids.iter().enumerate() {
        match client.get_item(id).await {
            Ok(item) if main_file_missing(item.main_file.as_deref()) => {
                mark_no_main_skipped(client, id).await;
                summary.skipped_no_main += 1;
                feedback(
                    client,
                    "LiteParse RAG extraction",
                    &format!(
                        "Extraction item {}/{} skipped (no main file): {id}",
                        index + 1,
                        ids.len()
                    ),
                    NotifyKind::Info,
                    false,
                )
                .await;
            }
            Ok(_) => eligible.push((index + 1, id.clone())),
            Err(error) => {
                let error = error.to_string();
                let _ = client
                    .set_item_extra(id, EXTRA, "status", Some("error"))
                    .await;
                let _ = client
                    .set_item_extra(id, EXTRA, "error", Some(&error))
                    .await;
                summary.failed.push(format!("{id}: {error}"));
                summary.retry_ids.push(id.clone());
            }
        }
    }
    if eligible.is_empty() {
        return summary;
    }
    let store = ModelStore::new(root);
    let paths = match store.ensure(client).await {
        Ok(paths) => paths,
        Err(e) => {
            for (_, id) in &eligible {
                let _ = client
                    .set_item_extra(id, EXTRA, "status", Some("error"))
                    .await;
                let _ =
                    client.set_item_extra(id, EXTRA, "error", Some(&e)).await;
                summary.failed.push(format!("{id}: {e}"));
                summary.retry_ids.push(id.clone());
            }
            feedback(
                client,
                "LiteParse RAG model setup failed",
                &format!("PP-OCRv6 models are unavailable: {e}"),
                NotifyKind::Error,
                true,
            )
            .await;
            return summary;
        }
    };
    let engine = match PpOcrV6OnnxEngine::load(paths) {
        Ok(engine) => Arc::new(engine),
        Err(e) => {
            for (_, id) in &eligible {
                let _ = client
                    .set_item_extra(id, EXTRA, "status", Some("error"))
                    .await;
                let _ =
                    client.set_item_extra(id, EXTRA, "error", Some(&e)).await;
                summary.failed.push(format!("{id}: {e}"));
                summary.retry_ids.push(id.clone());
            }
            return summary;
        }
    };
    for (ordinal, id) in eligible {
        feedback(
            client,
            "LiteParse RAG extraction",
            &format!(
                "Extraction item {ordinal}/{}: preparing {id}",
                ids.len()
            ),
            NotifyKind::Info,
            false,
        )
        .await;
        match process_one(
            client,
            root,
            &id,
            Arc::clone(&engine),
            ordinal,
            ids.len(),
        )
        .await
        {
            Ok(ProcessState::Completed) => {
                summary.completed += 1;
                feedback(
                    client,
                    "LiteParse RAG extraction",
                    &format!(
                        "Extraction item {ordinal}/{} completed: {id}",
                        ids.len()
                    ),
                    NotifyKind::Success,
                    false,
                )
                .await;
            }
            Ok(ProcessState::Unchanged) => {
                summary.unchanged += 1;
                feedback(
                    client,
                    "LiteParse RAG extraction",
                    &format!(
                        "Extraction item {ordinal}/{} unchanged: {id}",
                        ids.len()
                    ),
                    NotifyKind::Info,
                    false,
                )
                .await;
            }
            Ok(ProcessState::SkippedNoMain) => {
                mark_no_main_skipped(client, &id).await;
                summary.skipped_no_main += 1;
                feedback(
                    client,
                    "LiteParse RAG extraction",
                    &format!(
                        "Extraction item {ordinal}/{} skipped (no main file): {id}",
                        ids.len()
                    ),
                    NotifyKind::Info,
                    false,
                )
                .await;
            }
            Err(e) => {
                let _ = client
                    .set_item_extra(&id, EXTRA, "status", Some("error"))
                    .await;
                let _ =
                    client.set_item_extra(&id, EXTRA, "error", Some(&e)).await;
                feedback(
                    client,
                    "LiteParse RAG extraction failed",
                    &format!(
                        "Extraction item {ordinal}/{} failed: {id}: {e}",
                        ids.len()
                    ),
                    NotifyKind::Error,
                    false,
                )
                .await;
                summary.failed.push(format!("{id}: {e}"));
                summary.retry_ids.push(id.clone());
            }
        }
    }
    summary
}

pub(crate) enum ProcessState {
    Completed,
    Unchanged,
    SkippedNoMain,
}

pub(crate) async fn mark_no_main_skipped(client: &LocalrefClient, id: &str) {
    let _ = client.set_item_extra(id, EXTRA, "status", Some("skipped")).await;
    let _ = client
        .set_item_extra(id, EXTRA, "skip_reason", Some("no_main_file"))
        .await;
    let _ = client.set_item_extra(id, EXTRA, "error", None).await;
}

pub(crate) fn main_file_missing(main: Option<&str>) -> bool {
    main.is_none_or(|main| main.trim().is_empty())
}

pub(crate) async fn process_one(
    client: &LocalrefClient,
    root: &Path,
    id: &str,
    engine: Arc<PpOcrV6OnnxEngine>,
    ordinal: usize,
    total_items: usize,
) -> Result<ProcessState, String> {
    let item = client.get_item(id).await.map_err(|e| e.to_string())?;
    let Some(main) = item.main_file.as_deref() else {
        return Ok(ProcessState::SkippedNoMain);
    };
    if main_file_missing(Some(main)) {
        return Ok(ProcessState::SkippedNoMain);
    }
    let _ = client.set_item_extra(id, EXTRA, "skip_reason", None).await;
    let input = root.join(&item.object_path).join(main);
    if input
        .extension()
        .and_then(|x| x.to_str())
        .is_none_or(|x| !x.eq_ignore_ascii_case("pdf"))
    {
        return Err("v1 accepts PDF primary files only".into());
    }
    if !input.is_file() {
        return Err(format!("primary file is missing: {}", input.display()));
    }
    let source_sha = sha256_file(&input)?;
    let artifact = root.join(&item.object_path).join(ARTIFACT_DIR);
    if manifest_matches(&artifact.join("manifest.json"), &source_sha) {
        let _ =
            client.set_item_extra(id, EXTRA, "status", Some("ready")).await;
        let _ = client.set_item_extra(id, EXTRA, "skip_reason", None).await;
        return Ok(ProcessState::Unchanged);
    }
    let _ =
        client.set_item_extra(id, EXTRA, "status", Some("processing")).await;
    let result = write_artifacts(
        client,
        &input,
        &artifact,
        id,
        &item.title,
        &source_sha,
        engine,
        ordinal,
        total_items,
    )
    .await;
    match result {
        Ok(chunk_count) => {
            client
                .set_item_extra(id, EXTRA, "status", Some("ready"))
                .await
                .map_err(|e| e.to_string())?;
            client
                .set_item_extra(id, EXTRA, "source_sha256", Some(&source_sha))
                .await
                .map_err(|e| e.to_string())?;
            client
                .set_item_extra(id, EXTRA, "artifact_dir", Some(ARTIFACT_DIR))
                .await
                .map_err(|e| e.to_string())?;
            client
                .set_item_extra(id, EXTRA, "error", None)
                .await
                .map_err(|e| e.to_string())?;
            client
                .set_item_extra(id, EXTRA, "skip_reason", None)
                .await
                .map_err(|e| e.to_string())?;
            let _ = client
                .log(
                    PLUGIN,
                    LogLevel::Info,
                    &format!("generated {chunk_count} chunks"),
                )
                .await;
            Ok(ProcessState::Completed)
        }
        Err(e) => {
            let _ = client
                .set_item_extra(id, EXTRA, "status", Some("error"))
                .await;
            let _ = client.set_item_extra(id, EXTRA, "error", Some(&e)).await;
            Err(e)
        }
    }
}

pub(crate) async fn write_artifacts(
    client: &LocalrefClient,
    input: &Path,
    artifact: &Path,
    item_id: &str,
    title: &str,
    source_sha: &str,
    engine: Arc<PpOcrV6OnnxEngine>,
    ordinal: usize,
    total_items: usize,
) -> Result<usize, String> {
    let temporary =
        artifact.with_file_name(format!(".liteparse-rag-{}", nonce()));
    fs::create_dir_all(&temporary)
        .map_err(io("create temporary artifact directory"))?;
    let parse_result = async {
        let item_progress = format!("Extraction item {ordinal}/{total_items}");
        feedback(
            client,
            "LiteParse RAG extraction",
            &format!("{item_progress}: parsing PDF structure for {title}"),
            NotifyKind::Info,
            false,
        )
        .await;
        let config = LiteParseConfig {
            ocr_enabled: true,
            ocr_language: "ch".into(),
            dpi: 200.0,
            output_format: OutputFormat::Markdown,
            image_mode: ImageMode::Embed,
            emit_word_boxes: true,
            quiet: true,
            ..Default::default()
        };
        let provider = engine.provider();
        let parser = LiteParse::new(config).with_ocr_engine(engine.clone());
        let input_text = input.to_str().ok_or("PDF path is not valid UTF-8")?;
        let result = parser.parse(input_text).await.map_err(|e| format!("LiteParse failed: {e}"))?;
        feedback(
            client,
            "LiteParse RAG extraction",
            &format!("{item_progress}: PDF parsed; running semantic layout and formula recognition"),
            NotifyKind::Info,
            false,
        )
        .await;
        let page_dir = temporary.join("pages"); fs::create_dir_all(&page_dir).map_err(io("create page directory"))?;
        let region_dir = temporary.join("regions"); fs::create_dir_all(&region_dir).map_err(io("create semantic region directory"))?;
        let mut analyses_by_page = Vec::new();
        let screenshots = parser.screenshot(input_text, None).await.map_err(|e| format!("render page screenshot: {e}"))?;
        let page_count = screenshots.len();
        let mut formula_count = 0_usize;
        let mut formula_candidate_count = 0_usize;
        let mut formula_rejection_count = 0_usize;
        for (page_index, shot) in screenshots.into_iter().enumerate() {
            let page_ordinal = page_index + 1;
            progress(
                client,
                &format!("{item_progress}: analyzing layout on page {page_ordinal}/{page_count}"),
                NotifyKind::Info,
                page_ordinal == 1 || page_ordinal % 5 == 0 || page_ordinal == page_count,
            )
            .await;
            let page_image = image::load_from_memory(&shot.image_bytes)
                .map_err(|e| format!("decode page {} screenshot: {e}", shot.page_num))?
                .to_rgb8();
            let mut analysis = engine
                .analyze_page_sync(&page_image)
                .map_err(|e| format!("analyze layout on page {}: {e}", shot.page_num))?;
            save_visual_regions(
                &page_image,
                shot.page_num as usize,
                &region_dir,
                &mut analysis.regions,
            )?;
            formula_count += analysis.formulas.len();
            formula_candidate_count +=
                analysis.formulas.len() + analysis.formula_rejections.len();
            formula_rejection_count += analysis.formula_rejections.len();
            fs::write(page_dir.join(format!("page-{:04}.png", shot.page_num)), shot.image_bytes).map_err(io("write page screenshot"))?;
            analyses_by_page.push((shot.page_num as usize, analysis));
        }
        feedback(
            client,
            "LiteParse RAG extraction",
            &format!("{item_progress}: fused layout across {page_count} page(s); accepted {formula_count}/{formula_candidate_count} formula candidate(s) and rejected {formula_rejection_count}; writing artifacts"),
            NotifyKind::Info,
            false,
        )
        .await;
        let markdown = fuse_layout_markdown(&result.pages, &analyses_by_page);
        fs::write(temporary.join("document.md"), &markdown).map_err(io("write document Markdown"))?;
        let image_layout = result
            .images
            .iter()
            .map(|image| json!({"id": image.id, "page": image.page, "bbox": image.bbox, "format": image.format}))
            .collect::<Vec<_>>();
        let formula_layout = analyses_by_page
            .iter()
            .flat_map(|(page, analysis)| analysis.formulas.iter().map(move |formula| json!({
                "page": page,
                "bbox": formula.bbox,
                "latex": formula.latex,
                "layout_score": formula.layout_score,
            })))
            .collect::<Vec<_>>();
        let rejected_formula_layout = analyses_by_page
            .iter()
            .flat_map(|(page, analysis)| analysis.formula_rejections.iter().map(move |rejection| json!({
                "page": page,
                "bbox": rejection.bbox,
                "layout_score": rejection.layout_score,
                "reason": rejection.reason,
            })))
            .collect::<Vec<_>>();
        let mut formula_rejection_reasons = BTreeMap::<&str, usize>::new();
        for rejection in analyses_by_page
            .iter()
            .flat_map(|(_, analysis)| analysis.formula_rejections.iter())
        {
            *formula_rejection_reasons.entry(rejection.reason).or_default() += 1;
        }
        let semantic_layout = analyses_by_page.iter().map(|(page, analysis)| json!({
            "page": page,
            "screenshot_size": [analysis.width, analysis.height],
            "regions": analysis.regions,
        })).collect::<Vec<_>>();
        fs::write(temporary.join("layout.json"), serde_json::to_vec_pretty(&json!({"pages": &result.pages, "images": image_layout, "semantic_pages": semantic_layout, "formulas": formula_layout, "rejected_formulas": rejected_formula_layout})).map_err(|e| e.to_string())?).map_err(io("write layout"))?;
        let image_dir = temporary.join("images"); fs::create_dir_all(&image_dir).map_err(io("create image directory"))?;
        for image in result.images { fs::write(image_dir.join(format!("{}.{}", image.id, image.format)), image.bytes).map_err(io("write embedded image"))?; }
        let chunks = chunks_from_markdown(&markdown, item_id, title);
        write_jsonl(&temporary.join("chunks.jsonl"), &chunks)?;
        let manifest = json!({
            "schema_version": 3, "pipeline_revision": PIPELINE_REVISION, "source_sha256": source_sha, "source_file": input.file_name().and_then(|x| x.to_str()),
            "plugin": PLUGIN, "liteparse": "git:0c68b986", "ocr": {"engine": "PP-OCRv6-medium-onnx", "source": "Hugging Face", "revision": MODEL_REVISION, "provider": provider},
            "layout_analysis": {"engine": "PP-DocLayout-L-onnx", "provider": "cpu", "revision": LAYOUT_REVISION, "region_count": analyses_by_page.iter().map(|(_, analysis)| analysis.regions.len()).sum::<usize>()},
            "formula_recognition": {"engine": "PP-FormulaNet-S-onnx", "candidate_detector": "PP-DocLayout-L", "provider": "cpu", "count": formula_count, "candidate_count": formula_candidate_count, "accepted_count": formula_count, "rejected_count": formula_rejection_count, "rejection_reasons": formula_rejection_reasons},
            "document": "document.md", "layout": "layout.json", "chunks": "chunks.jsonl", "images": "images", "pages": "pages", "regions": "regions", "chunk_count": chunks.len(), "generated_unix_ms": unix_ms()
        });
        fs::write(temporary.join("manifest.json"), serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?).map_err(io("write manifest"))?;
        Ok::<usize, String>(chunks.len())
    }.await;
    match parse_result {
        Ok(count) => {
            replace_directory(&temporary, artifact)?;
            Ok(count)
        }
        Err(e) => {
            let _ = fs::remove_dir_all(&temporary);
            Err(e)
        }
    }
}
