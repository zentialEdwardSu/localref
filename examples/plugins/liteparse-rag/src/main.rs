//! Pure-Rust Localref paper extraction plugin.
//!
//! LiteParse is linked as a library and calls [`PpOcrV6OnnxEngine`] directly.
//! PP-DocLayout-L supplies semantic regions, PP-OCRv6 supplies OCR, and
//! PP-FormulaNet-S emits LaTeX only for regions classified as formulas;
//! there is deliberately no runtime Python, `lit` executable, HTTP OCR
//! service, or resident inference process.

use std::{
    collections::{BTreeMap, VecDeque},
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    pin::Pin,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use image::{
    Rgb, RgbImage,
    imageops::{FilterType, crop_imm, resize},
};
use liteparse::{
    LiteParse, LiteParseConfig, OutputFormat, ParsedPage, TextItem,
    config::ImageMode,
    ocr::{OcrEngine, OcrOptions, OcrResult},
};
use localref_core::config::LocalrefConfig;
use localref_plugin_sdk::{
    ActionContext, Invocation, LocalrefClient, LogLevel, NotifyKind,
    RunOutput, emit, parse_args,
};
use ort::{
    ep::{self, directml::PerformancePreference},
    session::Session,
    value::Tensor,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const PLUGIN: &str = "liteparse-rag";
const PIPELINE_REVISION: &str = "4-formula-quality-1";
const EXTRA: &str = "liteparse_rag";
const ARTIFACT_DIR: &str = "llm/liteparse-rag";
const MODEL_BASE: &str = "https://huggingface.co";
const DETECTOR_REPO: &str = "PaddlePaddle/PP-OCRv6_medium_det_onnx";
const RECOGNIZER_REPO: &str = "PaddlePaddle/PP-OCRv6_medium_rec_onnx";
const FORMULA_REPO: &str = "x3zvawq/paddleocr-js-onnx";
const LAYOUT_REPO: &str = "x3zvawq/paddleocr-js-onnx";
const FORMULA_CONFIG_REPO: &str = "PaddlePaddle/PP-FormulaNet-S";
// Pin the Hugging Face commits, rather than resolving the mutable `main` refs
// during a user's first extraction. The SHA-256 table below remains the final
// content-integrity check after Hugging Face's redirect/CDN layer.
const DETECTOR_REVISION: &str = "61323801669c338b7891481ec7bac61ce31b576a";
const RECOGNIZER_REVISION: &str = "50c7eacafc52fa7bcf4194e8cd08e46f8558504b";
const FORMULA_REVISION: &str = "51c2133b5a7ea27b795fa8c400fdbfbd5337dd6a";
const LAYOUT_REVISION: &str = "51c2133b5a7ea27b795fa8c400fdbfbd5337dd6a";
const FORMULA_CONFIG_REVISION: &str =
    "0572450e501be9eb1b1cdb7e00fccf4b22fab4df";
const OCR_MODEL_REVISION: &str = "huggingface:detector@61323801669c338b7891481ec7bac61ce31b576a;recognizer@50c7eacafc52fa7bcf4194e8cd08e46f8558504b";
const PRE_LAYOUT_MODEL_REVISION: &str = "huggingface:detector@61323801669c338b7891481ec7bac61ce31b576a;recognizer@50c7eacafc52fa7bcf4194e8cd08e46f8558504b;formula@51c2133b5a7ea27b795fa8c400fdbfbd5337dd6a;formula-config@0572450e501be9eb1b1cdb7e00fccf4b22fab4df";
const MODEL_REVISION: &str = "huggingface:detector@61323801669c338b7891481ec7bac61ce31b576a;recognizer@50c7eacafc52fa7bcf4194e8cd08e46f8558504b;formula@51c2133b5a7ea27b795fa8c400fdbfbd5337dd6a;formula-config@0572450e501be9eb1b1cdb7e00fccf4b22fab4df;layout@51c2133b5a7ea27b795fa8c400fdbfbd5337dd6a";
const CHUNK_BYTES: usize = 1_600;

fn expected_model_sha(model: &str, file: &str) -> &'static str {
    match (model, file) {
        ("detector", "inference.onnx") => {
            "eb13b44b25bb36f89528b68720af8a61d9cf381176107f465db1757b65d086e1"
        }
        ("detector", "inference.yml") => {
            "7298d5ead546584af2504d03355f881ac7a7bc0eb1e282d3e159277c1d0af871"
        }
        ("detector", "inference.json") => {
            "0f1a7ec35da36173529c7a60238b7f7919e3831929c3f700ad90ad4896adecd5"
        }
        ("recognizer", "inference.onnx") => {
            "9c09abf0957f7968c7586464b7397b84ad2387a0497a351af40e9acc71b673ba"
        }
        ("recognizer", "inference.yml") => {
            "991b700facf5b50a7de193468207d5f4255b538dde0d312ae3b7c7a9b6873129"
        }
        ("recognizer", "inference.json") => {
            "0b2e25e990bd072f1bf77d59d67d508bce6c4bd44af6624e0fb27d6da2cd00e8"
        }
        ("formula", "inference.onnx") => {
            "4362211b5bfdf7aa9749b28f8cd5240cb22d90eb5d79045facec8fcb317a2659"
        }
        ("formula", "config.json") => {
            "ea32742b976ba34711042cac4e46206f114067949448c2bd8dec60a44d3de1fb"
        }
        ("layout", "inference.onnx") => {
            "01fd1a44fbea5b0a76302de356c1518250cbd34ee82833ac04d907034c1376e1"
        }
        ("layout", "inference.yml") => {
            "fbdbb903efd3d82db5800f9ae3e2477d2d84525956ef410ffe8e64bbaad02fa5"
        }
        _ => unreachable!(
            "only fixed OCR, formula, and layout model assets are requested"
        ),
    }
}

mod artifacts;
mod formula;
mod inference;
mod models;
mod storage;
mod workflow;

pub(crate) use artifacts::*;
pub(crate) use formula::*;
pub(crate) use inference::*;
pub(crate) use models::*;
pub(crate) use storage::*;
pub(crate) use workflow::*;

#[tokio::main]
async fn main() {
    let Some(invocation) = parse_args(std::env::args().skip(1)) else {
        emit(&RunOutput::error(
            "usage: liteparse-rag <run|hook|cron> ... --endpoint ...",
        ));
        return;
    };
    match invocation {
        Invocation::Manifest => println!(
            "{PLUGIN} — LiteParse + PP-DocLayout-L + PP-OCRv6 ONNX RAG extraction"
        ),
        Invocation::Run { action, endpoint, selected, active, params } => {
            let ctx = ActionContext {
                selected,
                active,
                params,
                client: LocalrefClient::new(endpoint),
            };
            emit(&run_action(&action, &ctx).await);
        }
        Invocation::Hook { event, endpoint, item, .. } => {
            let output = match (event.as_str(), item) {
                ("item_imported" | "item_file_added", Some(id)) => {
                    enqueue_hook(&endpoint, &id).await
                }
                _ => RunOutput::done(),
            };
            emit(&output);
        }
        Invocation::Cron { job, endpoint } => {
            let output = if job == "process_queue" {
                process_queue(&endpoint).await
            } else {
                RunOutput::error(format!("unknown cron job: {job}"))
            };
            emit(&output);
        }
    }
}

#[cfg(test)]
mod tests;
