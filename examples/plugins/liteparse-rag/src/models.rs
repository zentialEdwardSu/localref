//! Pinned model acquisition, integrity checking, and local model caching.

use crate::*;

#[derive(Clone)]
pub(crate) struct ModelPaths {
    pub(crate) detector: PathBuf,
    pub(crate) recognizer: PathBuf,
    pub(crate) recognizer_config: PathBuf,
    pub(crate) formula: PathBuf,
    pub(crate) formula_config: PathBuf,
    pub(crate) layout: PathBuf,
    pub(crate) layout_config: PathBuf,
}
pub(crate) struct ModelStore {
    root: PathBuf,
}
impl ModelStore {
    pub(crate) fn new(library: &Path) -> Self {
        Self { root: library.join(".localref").join(PLUGIN).join("models") }
    }
    pub(crate) async fn ensure(
        &self,
        client: &LocalrefClient,
    ) -> Result<ModelPaths, String> {
        fs::create_dir_all(&self.root).map_err(io("create model cache"))?;
        let lock_path = self.root.join("model-lock.json");
        let mut lock: ModelLock = read_json(&lock_path).unwrap_or_default();
        if lock.revision.is_empty()
            || lock.revision == "master"
            || lock.revision == OCR_MODEL_REVISION
            || lock.revision == PRE_LAYOUT_MODEL_REVISION
        {
            lock.revision = MODEL_REVISION.to_string();
        }
        if lock.revision != MODEL_REVISION {
            return Err(format!(
                "cached model revision {} does not match required {MODEL_REVISION}",
                lock.revision
            ));
        }
        let http = reqwest::Client::new();
        let det = self
            .ensure_model(
                "detector",
                DETECTOR_REPO,
                DETECTOR_REVISION,
                &mut lock,
                client,
                &http,
                false,
                &[
                    ("inference.onnx", "inference.onnx"),
                    ("inference.yml", "inference.yml"),
                    ("inference.json", "inference.json"),
                ],
            )
            .await?;
        let rec = self
            .ensure_model(
                "recognizer",
                RECOGNIZER_REPO,
                RECOGNIZER_REVISION,
                &mut lock,
                client,
                &http,
                false,
                &[
                    ("inference.onnx", "inference.onnx"),
                    ("inference.yml", "inference.yml"),
                    ("inference.json", "inference.json"),
                ],
            )
            .await?;
        let formula = self
            .ensure_model(
                "formula",
                FORMULA_REPO,
                FORMULA_REVISION,
                &mut lock,
                client,
                &http,
                false,
                &[(
                    "inference.onnx",
                    "PP_FormulaNet_S/PP-FormulaNet-S_infer.onnx",
                )],
            )
            .await?;
        self.ensure_model(
            "formula",
            FORMULA_CONFIG_REPO,
            FORMULA_CONFIG_REVISION,
            &mut lock,
            client,
            &http,
            false,
            &[("config.json", "config.json")],
        )
        .await?;
        let layout = self
            .ensure_model(
                "layout",
                LAYOUT_REPO,
                LAYOUT_REVISION,
                &mut lock,
                client,
                &http,
                false,
                &[
                    (
                        "inference.onnx",
                        "pp_doclayout_l/PP-DocLayout-L_infer.onnx",
                    ),
                    (
                        "inference.yml",
                        "pp_doclayout_l/PP-DocLayout-L_inference.yml",
                    ),
                ],
            )
            .await?;
        write_json(&lock_path, &lock)?;
        Ok(ModelPaths {
            detector: det.join("inference.onnx"),
            recognizer: rec.join("inference.onnx"),
            recognizer_config: rec.join("inference.yml"),
            formula: formula.join("inference.onnx"),
            formula_config: formula.join("config.json"),
            layout: layout.join("inference.onnx"),
            layout_config: layout.join("inference.yml"),
        })
    }
    async fn ensure_model(
        &self,
        name: &str,
        repo: &str,
        revision: &str,
        lock: &mut ModelLock,
        client: &LocalrefClient,
        http: &reqwest::Client,
        allow_download: bool,
        assets: &[(&str, &str)],
    ) -> Result<PathBuf, String> {
        let directory = self.root.join(name);
        fs::create_dir_all(&directory)
            .map_err(io("create model directory"))?;
        for &(file, remote_file) in assets {
            let path = directory.join(file);
            let key = format!("{name}/{file}");
            if path.exists() {
                feedback(
                    client,
                    "LiteParse RAG model verification",
                    &format!("Verifying cached {key}…"),
                    NotifyKind::Info,
                    false,
                )
                .await;
                let actual = sha256_file(&path)?;
                let expected = expected_model_sha(name, file);
                if actual != expected {
                    if !allow_download {
                        return Err(format!(
                            "cached model checksum mismatch: {}. Re-download it from the Download models window.",
                            path.display()
                        ));
                    }
                } else {
                    if let Some(expected) = lock.files.get(&key) {
                        if expected != &actual {
                            return Err(format!(
                                "cached model checksum mismatch: {}. Re-download it from the Download models window.",
                                path.display()
                            ));
                        }
                    } else {
                        lock.files.insert(key, actual);
                    }
                    feedback(
                        client,
                        "LiteParse RAG model verification",
                        &format!("Verified cached {name}/{file}"),
                        NotifyKind::Info,
                        false,
                    )
                    .await;
                    continue;
                }
            }
            if !allow_download {
                return Err(format!(
                    "model file is missing: {}. Automatic downloads are disabled; open liteparse-rag: Download OCR, layout, and formula models.",
                    path.display()
                ));
            }
            let url = model_download_url(repo, revision, remote_file);
            feedback(
                client,
                "LiteParse RAG model download",
                &format!("Downloading and verifying {name}/{file}…"),
                NotifyKind::Info,
                false,
            )
            .await;
            let mut response = http
                .get(&url)
                .send()
                .await
                .map_err(|e| format!("download {url}: {e}"))?;
            if !response.status().is_success() {
                return Err(format!(
                    "download {url}: HTTP {}",
                    response.status()
                ));
            }
            let content_length = response.content_length();
            let temporary = path.with_extension("download");
            let mut output = fs::File::create(&temporary)
                .map_err(io("create downloaded model"))?;
            let mut digest = Sha256::new();
            let mut downloaded = 0_u64;
            let mut next_percent = 10_u64;
            let mut next_unbounded_report = 16 * 1024 * 1024_u64;
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|e| format!("read {url}: {e}"))?
            {
                digest.update(&chunk);
                output
                    .write_all(&chunk)
                    .map_err(io("write downloaded model"))?;
                downloaded = downloaded.saturating_add(chunk.len() as u64);
                let report = if let Some(total) =
                    content_length.filter(|x| *x > 0)
                {
                    let percent = downloaded.saturating_mul(100) / total;
                    if percent >= next_percent && percent < 100 {
                        next_percent = (percent / 10 + 1).saturating_mul(10);
                        Some(format!(
                            "Downloading {name}/{file}: {percent}% ({}/{})",
                            human_bytes(downloaded),
                            human_bytes(total)
                        ))
                    } else {
                        None
                    }
                } else if downloaded >= next_unbounded_report {
                    next_unbounded_report =
                        downloaded.saturating_add(16 * 1024 * 1024);
                    Some(format!(
                        "Downloading {name}/{file}: {} received",
                        human_bytes(downloaded)
                    ))
                } else {
                    None
                };
                if let Some(message) = report {
                    progress(client, &message, NotifyKind::Info, true).await;
                }
            }
            output.flush().map_err(io("flush downloaded model"))?;
            drop(output);
            let actual = format!("{:x}", digest.finalize());
            if actual != expected_model_sha(name, file) {
                let _ = fs::remove_file(&temporary);
                return Err(format!(
                    "downloaded model checksum mismatch: {key}"
                ));
            }
            if path.exists() {
                fs::remove_file(&path)
                    .map_err(io("replace invalid downloaded model"))?;
            }
            fs::rename(&temporary, &path)
                .map_err(io("publish downloaded model"))?;
            lock.files.insert(key, actual);
            feedback(
                client,
                "LiteParse RAG model download",
                &format!(
                    "Verified {name}/{file} ({})",
                    human_bytes(downloaded)
                ),
                NotifyKind::Info,
                false,
            )
            .await;
        }
        Ok(directory)
    }

    /// Explicit, user-triggered model installation. Extraction never calls
    /// this method, so normal OCR remains entirely offline.
    pub(crate) async fn download(
        &self,
        client: &LocalrefClient,
        proxy: Option<&str>,
    ) -> Result<(), String> {
        fs::create_dir_all(&self.root).map_err(io("create model cache"))?;
        let lock_path = self.root.join("model-lock.json");
        let mut lock: ModelLock = read_json(&lock_path).unwrap_or_default();
        if lock.revision.is_empty()
            || lock.revision == "master"
            || lock.revision == OCR_MODEL_REVISION
            || lock.revision == PRE_LAYOUT_MODEL_REVISION
        {
            lock.revision = MODEL_REVISION.to_string();
        }
        if lock.revision != MODEL_REVISION {
            return Err(format!(
                "cached model revision {} does not match required {MODEL_REVISION}",
                lock.revision
            ));
        }
        let mut builder = reqwest::Client::builder();
        if let Some(proxy) = proxy {
            let configured = reqwest::Proxy::all(proxy)
                .map_err(|e| format!("invalid proxy configuration: {e}"))?;
            builder = builder.proxy(configured);
        }
        let http = builder
            .build()
            .map_err(|e| format!("create model download client: {e}"))?;
        feedback(
            client,
            "LiteParse RAG model download",
            if proxy.is_some() {
                "Downloading PP-OCRv6, PP-DocLayout-L, and PP-FormulaNet-S through the configured proxy…"
            } else {
                "Downloading PP-OCRv6, PP-DocLayout-L, and PP-FormulaNet-S from Hugging Face…"
            },
            NotifyKind::Info,
            true,
        )
        .await;
        self.ensure_model(
            "detector",
            DETECTOR_REPO,
            DETECTOR_REVISION,
            &mut lock,
            client,
            &http,
            true,
            &[
                ("inference.onnx", "inference.onnx"),
                ("inference.yml", "inference.yml"),
                ("inference.json", "inference.json"),
            ],
        )
        .await?;
        self.ensure_model(
            "recognizer",
            RECOGNIZER_REPO,
            RECOGNIZER_REVISION,
            &mut lock,
            client,
            &http,
            true,
            &[
                ("inference.onnx", "inference.onnx"),
                ("inference.yml", "inference.yml"),
                ("inference.json", "inference.json"),
            ],
        )
        .await?;
        self.ensure_model(
            "formula",
            FORMULA_REPO,
            FORMULA_REVISION,
            &mut lock,
            client,
            &http,
            true,
            &[(
                "inference.onnx",
                "PP_FormulaNet_S/PP-FormulaNet-S_infer.onnx",
            )],
        )
        .await?;
        self.ensure_model(
            "formula",
            FORMULA_CONFIG_REPO,
            FORMULA_CONFIG_REVISION,
            &mut lock,
            client,
            &http,
            true,
            &[("config.json", "config.json")],
        )
        .await?;
        self.ensure_model(
            "layout",
            LAYOUT_REPO,
            LAYOUT_REVISION,
            &mut lock,
            client,
            &http,
            true,
            &[
                ("inference.onnx", "pp_doclayout_l/PP-DocLayout-L_infer.onnx"),
                (
                    "inference.yml",
                    "pp_doclayout_l/PP-DocLayout-L_inference.yml",
                ),
            ],
        )
        .await?;
        write_json(&lock_path, &lock)?;
        feedback(
            client,
            "LiteParse RAG models ready",
            "PP-OCRv6 medium, PP-FormulaNet-S, and PP-DocLayout-L models were downloaded and verified successfully.",
            NotifyKind::Success,
            true,
        )
        .await;
        Ok(())
    }
}

pub(crate) fn model_download_url(
    repo: &str,
    revision: &str,
    file: &str,
) -> String {
    format!("{MODEL_BASE}/{repo}/resolve/{revision}/{file}")
}

pub(crate) fn human_bytes(bytes: u64) -> String {
    const MIB: f64 = 1024.0 * 1024.0;
    if bytes < 1024 * 1024 {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MiB", bytes as f64 / MIB)
    }
}
#[derive(Default, Serialize, Deserialize)]
pub(crate) struct ModelLock {
    revision: String,
    files: BTreeMap<String, String>,
}
