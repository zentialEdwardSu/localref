//! Cross-module regression tests for the plugin pipeline.

use super::*;
#[test]
fn components_find_separate_regions() {
    let scores = vec![
        0.0, 0.9, 0.9, 0.0, 0.0, 0.9, 0.9, 0.0, 0.0, 0.0, 0.0, 0.8, 0.0, 0.0,
        0.8, 0.8,
    ];
    assert_eq!(components(&scores, 4, 4, 0.3).len(), 0);
}
#[test]
fn chunks_keep_item_identity() {
    let chunks =
        chunks_from_markdown(&"x".repeat(CHUNK_BYTES + 1), "lr:x", "Paper");
    assert_eq!(chunks.len(), 2);
    assert_eq!(chunks[0].item_id, "lr:x");
}

#[test]
fn batch_summary_separates_unchanged_and_missing_main_file() {
    let summary = BatchSummary {
        completed: 2,
        unchanged: 3,
        skipped_no_main: 4,
        ..Default::default()
    };
    assert_eq!(summary.handled(), 9);
    assert_eq!(
        summary.message(),
        "LiteParse RAG: 2 processed, 3 unchanged, 4 skipped (no main file)"
    );
    assert!(main_file_missing(None));
    assert!(main_file_missing(Some("  ")));
    assert!(!main_file_missing(Some("paper.pdf")));
}

#[test]
fn unchanged_requires_matching_manifest_and_complete_artifacts() {
    let artifact = std::env::temp_dir()
        .join(format!("liteparse-rag-manifest-test-{}", nonce()));
    fs::create_dir_all(&artifact).unwrap();
    for directory in ["images", "pages", "regions"] {
        fs::create_dir(artifact.join(directory)).unwrap();
    }
    for file in ["document.md", "layout.json", "chunks.jsonl"] {
        fs::write(artifact.join(file), b"test").unwrap();
    }
    fs::write(
        artifact.join("manifest.json"),
        serde_json::to_vec(&json!({
            "source_sha256": "source-sha",
            "pipeline_revision": PIPELINE_REVISION,
            "ocr": {"revision": MODEL_REVISION},
            "formula_recognition": {"engine": "PP-FormulaNet-S-onnx"},
            "layout_analysis": {"engine": "PP-DocLayout-L-onnx"}
        }))
        .unwrap(),
    )
    .unwrap();
    let manifest = artifact.join("manifest.json");
    assert!(manifest_matches(&manifest, "source-sha"));
    assert!(!manifest_matches(&manifest, "different-source"));
    fs::remove_file(artifact.join("document.md")).unwrap();
    assert!(!manifest_matches(&manifest, "source-sha"));
    fs::remove_dir_all(artifact).unwrap();
}

#[test]
fn hash_is_stable() {
    assert_eq!(
        sha256_bytes(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn download_progress_formats_byte_counts_for_people() {
    assert_eq!(human_bytes(512 * 1024), "512.0 KiB");
    assert_eq!(human_bytes(309 * 1024 * 1024), "309.0 MiB");
}
#[test]
fn model_assets_have_pinned_sha256() {
    assert_eq!(expected_model_sha("detector", "inference.onnx").len(), 64);
    assert_eq!(expected_model_sha("recognizer", "inference.json").len(), 64);
    assert_eq!(
        expected_model_sha("layout", "inference.onnx"),
        "01fd1a44fbea5b0a76302de356c1518250cbd34ee82833ac04d907034c1376e1"
    );
}

#[test]
fn model_downloads_use_pinned_hugging_face_commits() {
    assert_eq!(DETECTOR_REVISION.len(), 40);
    assert_eq!(RECOGNIZER_REVISION.len(), 40);
    assert_eq!(
        model_download_url(DETECTOR_REPO, DETECTOR_REVISION, "inference.onnx"),
        "https://huggingface.co/PaddlePaddle/PP-OCRv6_medium_det_onnx/resolve/61323801669c338b7891481ec7bac61ce31b576a/inference.onnx"
    );
    assert_eq!(
        model_download_url(
            FORMULA_REPO,
            FORMULA_REVISION,
            "PP_FormulaNet_S/PP-FormulaNet-S_infer.onnx"
        ),
        "https://huggingface.co/x3zvawq/paddleocr-js-onnx/resolve/51c2133b5a7ea27b795fa8c400fdbfbd5337dd6a/PP_FormulaNet_S/PP-FormulaNet-S_infer.onnx"
    );
    assert_eq!(
        model_download_url(
            LAYOUT_REPO,
            LAYOUT_REVISION,
            "pp_doclayout_l/PP-DocLayout-L_infer.onnx"
        ),
        "https://huggingface.co/x3zvawq/paddleocr-js-onnx/resolve/51c2133b5a7ea27b795fa8c400fdbfbd5337dd6a/pp_doclayout_l/PP-DocLayout-L_infer.onnx"
    );
}

#[test]
fn recognizer_dictionary_ignores_label_pipeline_keys() {
    let yaml: serde_yaml::Value = serde_yaml::from_str(
        r#"
PreProcess:
  KeepKeys:
    keep_keys: [image, label_ctc, label_gtc]
PostProcess:
  name: CTCLabelDecode
  character_dict: [a, b, c]
"#,
    )
    .unwrap();
    let mut dictionary = Vec::new();
    collect_dictionary(&yaml, &mut dictionary);
    assert_eq!(dictionary, ["a", "b", "c"]);
}

#[test]
fn layout_fusion_orders_the_left_column_before_the_right_column() {
    let blocks = vec![
        FusedBlock { bbox: [550, 20, 900, 80], content: "right".into() },
        FusedBlock { bbox: [50, 100, 450, 160], content: "left".into() },
    ];
    let ordered = order_fused_blocks(blocks, 1000);
    assert_eq!(ordered[0].content, "left");
    assert_eq!(ordered[1].content, "right");
}

#[test]
fn chart_region_wins_over_a_duplicate_generic_image_region() {
    let regions = vec![
        LayoutRegion {
            label: "image".into(),
            score: 0.94,
            bbox: [10, 20, 300, 400],
            asset: None,
        },
        LayoutRegion {
            label: "chart".into(),
            score: 0.62,
            bbox: [11, 21, 299, 399],
            asset: None,
        },
    ];
    assert_eq!(visual_region_keep_mask(&regions), [false, true]);
}

#[test]
fn adjacent_detected_heading_lines_are_fused() {
    let blocks = vec![
        FusedBlock {
            bbox: [100, 100, 500, 130],
            content: "## C. Channel Models in the".into(),
        },
        FusedBlock {
            bbox: [100, 132, 500, 162],
            content: "## Concise Version of Scenarios".into(),
        },
    ];
    let merged = merge_adjacent_headings(blocks, 1000);
    assert_eq!(merged.len(), 1);
    assert_eq!(
        merged[0].content,
        "## C. Channel Models in the Concise Version of Scenarios"
    );
}

#[test]
fn layout_region_text_preserves_headings_and_dehyphenates_wrapped_text() {
    let first = TextItem {
        text: "Inter-".into(),
        x: 10.0,
        y: 10.0,
        width: 50.0,
        height: 10.0,
        ..Default::default()
    };
    let second = TextItem {
        text: "ference model".into(),
        x: 10.0,
        y: 25.0,
        width: 70.0,
        height: 10.0,
        ..Default::default()
    };
    assert_eq!(
        format_region_text("paragraph_title", &[&first, &second]),
        Some("## Interference model".into())
    );
}

#[test]
fn formula_spacing_keeps_words_and_latex_spaces() {
    assert_eq!(
        normalize_formula_spacing(
            r"\zeta _ { 0 } = \frac { 2 z } { \pi } \ \  d z"
        ),
        r"\zeta_{0}=\frac{2z}{\pi}\ \ d z"
    );
}

#[test]
fn formula_quality_accepts_balanced_math() {
    assert_eq!(
        implausible_formula_reason(
            r"\begin{aligned}H_{n_F}&=G_{n_F}\times_1 A_h(\theta)\\Y&=X\odot H+Z\end{aligned}"
        ),
        None
    );
    assert_eq!(
        implausible_formula_reason(
            r"\zeta_{0}(\nu)=-{\frac{\nu\varrho^{-2\nu}}{\pi}}\int_{\mu}^{\infty}d\omega\int_{C_{+}}d z{\frac{2z^{2}}{(z^{2}+\omega^{2})^{\nu+1}}}\ \ {vec\Psi}(\omega;z)e^{i\epsilon z}\quad,"
        ),
        None
    );
}

#[test]
fn formula_quality_rejects_observed_decoder_noise() {
    assert_eq!(
        implausible_formula_reason(r"\frac{x}{y"),
        Some("unbalanced_braces")
    );
    assert_eq!(
        implausible_formula_reason(r"\begin{array}x+y"),
        Some("unbalanced_environment")
    );
    assert_eq!(
        implausible_formula_reason(
            r"\mathrm{s u b f u c t i o n s~w i t h~r e s p e c t}"
        ),
        Some("prose_like_output")
    );
    let repeated_spacing = format!("x={}", r"\quad".repeat(13));
    assert_eq!(
        implausible_formula_reason(&repeated_spacing),
        Some("excessive_spacing")
    );
    assert_eq!(
        implausible_formula_reason(r"\begin array{x+y}\end array"),
        Some("malformed_environment")
    );
    assert_eq!(
        implausible_formula_reason(r"{mathtt{a s}}+x"),
        Some("missing_command_escape")
    );
    assert_eq!(
        implausible_formula_reason(r"\mathttrightarrow{x}+y"),
        Some("unsupported_command")
    );
    assert_eq!(
        implausible_formula_reason(r"\mathsfmathsf{x}+y"),
        Some("unsupported_command")
    );
    assert_eq!(
        implausible_formula_reason(r"(n_{\mathrm{h}},n_{\mathrm{v}})\ !!!"),
        Some("repeated_punctuation")
    );
    assert_eq!(
        implausible_formula_reason(r"\_{\cdot}(i_1,i_2)"),
        Some("malformed_math_prefix")
    );
    assert_eq!(
        implausible_formula_reason(r"\Delta\dot{bar f}"),
        Some("missing_command_escape")
    );
}

#[test]
fn formula_bbox_padding_does_not_swallow_neighboring_lines() {
    assert_eq!(
        pad_formula_bbox([100, 100, 200, 150], 1000, 1000),
        [88, 94, 212, 156]
    );
    assert_eq!(pad_formula_bbox([1, 1, 20, 9], 100, 100), [0, 0, 22, 10]);
}

#[test]
fn rejected_formula_falls_back_to_native_pdf_text() {
    let left = TextItem {
        text: "x".into(),
        x: 10.0,
        y: 10.0,
        width: 5.0,
        height: 10.0,
        ..Default::default()
    };
    let right = TextItem {
        text: "= y".into(),
        x: 20.0,
        y: 10.0,
        width: 15.0,
        height: 10.0,
        ..Default::default()
    };
    assert_eq!(
        format_formula_text_fallback(&[&left, &right]),
        Some("x = y".into())
    );
}

#[test]
#[ignore = "requires the manually prepared 309 MB PP-FormulaNet-S ONNX fixture"]
fn formula_onnx_matches_official_sample() {
    let model_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(".model-work")
        .join("PP-FormulaNet-S");
    let image = image::open(model_dir.join("sample.png")).unwrap().to_rgb8();
    let mut session =
        create_cpu_session(&model_dir.join("community.onnx")).unwrap();
    let tokenizer =
        FormulaTokenizer::load(&model_dir.join("config.json")).unwrap();
    let ids = recognize_formula(&mut session, &image).unwrap();
    assert_eq!(
        tokenizer.decode(&ids).unwrap(),
        r"\zeta_{0}(\nu)=-{\frac{\nu\varrho^{-2\nu}}{\pi}}\int_{\mu}^{\infty}d\omega\int_{C_{+}}d z{\frac{2z^{2}}{(z^{2}+\omega^{2})^{\nu+1}}}\ \ {vec\Psi}(\omega;z)e^{i\epsilon z}\quad,"
    );
}

#[test]
#[ignore = "requires PP-OCRv6, PP-DocLayout-L, and PP-FormulaNet-S model fixtures"]
fn doclayout_routes_the_official_formula_sample_to_formulanet() {
    let ocr_dir = PathBuf::from(
        std::env::var("LOCALREF_TEST_OCR_MODELS")
            .expect("set LOCALREF_TEST_OCR_MODELS"),
    );
    let formula_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(".model-work")
        .join("PP-FormulaNet-S");
    let layout_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(".model-work")
        .join("PP-DocLayout-L");
    let engine = PpOcrV6OnnxEngine::load(ModelPaths {
        detector: ocr_dir.join("detector/inference.onnx"),
        recognizer: ocr_dir.join("recognizer/inference.onnx"),
        recognizer_config: ocr_dir.join("recognizer/inference.yml"),
        formula: formula_dir.join("community.onnx"),
        formula_config: formula_dir.join("config.json"),
        layout: layout_dir.join("inference.onnx"),
        layout_config: layout_dir.join("inference.yml"),
    })
    .unwrap();
    let image = image::open(formula_dir.join("sample.png")).unwrap().to_rgb8();
    let analysis = engine.analyze_page_sync(&image).unwrap();
    assert!(
        analysis
            .formulas
            .iter()
            .any(|formula| formula.latex.contains("\\zeta_{0}")),
        "PP-DocLayout-L did not route the formula candidate: {:?}",
        analysis.regions
    );
}

#[test]
#[ignore = "requires LOCALREF_TEST_MODELS, LOCALREF_TEST_FORMULA_PAGE, and installed ONNX models"]
fn formula_quality_filters_a_real_document_page() {
    let model_dir = PathBuf::from(
        std::env::var("LOCALREF_TEST_MODELS")
            .expect("set LOCALREF_TEST_MODELS"),
    );
    let page = image::open(
        std::env::var("LOCALREF_TEST_FORMULA_PAGE")
            .expect("set LOCALREF_TEST_FORMULA_PAGE"),
    )
    .unwrap()
    .to_rgb8();
    let engine = PpOcrV6OnnxEngine::load(ModelPaths {
        detector: model_dir.join("detector/inference.onnx"),
        recognizer: model_dir.join("recognizer/inference.onnx"),
        recognizer_config: model_dir.join("recognizer/inference.yml"),
        formula: model_dir.join("formula/inference.onnx"),
        formula_config: model_dir.join("formula/config.json"),
        layout: model_dir.join("layout/inference.onnx"),
        layout_config: model_dir.join("layout/inference.yml"),
    })
    .unwrap();
    let analysis = engine.analyze_page_sync(&page).unwrap();
    eprintln!(
        "formula candidates: {}, accepted: {}, rejected: {}",
        analysis.formulas.len() + analysis.formula_rejections.len(),
        analysis.formulas.len(),
        analysis.formula_rejections.len()
    );
    for formula in &analysis.formulas {
        eprintln!(
            "accepted {:?}: {}",
            formula.bbox,
            formula.latex.chars().take(180).collect::<String>()
        );
    }
    for rejection in &analysis.formula_rejections {
        eprintln!("rejected {:?}: {}", rejection.bbox, rejection.reason);
    }
    assert!(
        !analysis.formula_rejections.is_empty(),
        "the regression page should exercise the formula quality gate"
    );
    assert!(analysis.formulas.iter().all(|formula| {
        implausible_formula_reason(&formula.latex).is_none()
    }));
}

#[test]
#[ignore = "requires LOCALREF_TEST_LAYOUT_PAGE and the PP-DocLayout-L fixture"]
fn doclayout_classifies_a_real_document_page() {
    let model_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(".model-work")
        .join("PP-DocLayout-L");
    let page = image::open(
        std::env::var("LOCALREF_TEST_LAYOUT_PAGE")
            .expect("set LOCALREF_TEST_LAYOUT_PAGE"),
    )
    .unwrap()
    .to_rgb8();
    let labels = load_layout_labels(&model_dir.join("inference.yml")).unwrap();
    let mut session =
        create_cpu_session(&model_dir.join("inference.onnx")).unwrap();
    let regions = detect_layout(&mut session, &page, &labels).unwrap();
    let expected = std::env::var("LOCALREF_TEST_LAYOUT_EXPECT")
        .unwrap_or_else(|_| "text".into());
    assert!(
        regions.iter().any(|region| region.label == expected),
        "expected {expected}, got {regions:?}"
    );
    assert!(
        regions.iter().all(|region| region.label != "formula"),
        "non-formula page was routed to FormulaNet: {regions:?}"
    );
}

#[test]
fn proxy_address_and_port_form_an_http_proxy() {
    let params = std::collections::HashMap::from([
        ("proxy_address".to_string(), "127.0.0.1".to_string()),
        ("proxy_port".to_string(), "7890".to_string()),
    ]);
    assert_eq!(
        proxy_from_params(&params).unwrap().as_deref(),
        Some("http://127.0.0.1:7890/")
    );
}

#[test]
fn proxy_port_requires_an_address() {
    let params = std::collections::HashMap::from([(
        "proxy_port".to_string(),
        "7890".to_string(),
    )]);
    assert!(proxy_from_params(&params).is_err());
}
