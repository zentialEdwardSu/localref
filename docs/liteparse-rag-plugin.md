# LiteParse RAG Plugin

`liteparse-rag` is a native Localref plugin for PDF papers. It links
LiteParse as a Rust library and supplies a Rust PP-OCRv6 medium ONNX engine
through LiteParse's `OcrEngine` interface. PP-DocLayout-L detects 23 semantic
region types and its `formula` regions alone are sent to PP-FormulaNet-S for
LaTeX recognition. Recognized LaTeX must pass structural, repetition, and
prose-like-output checks before it enters the document; rejected candidates
fall back to the PDF's native text geometry instead of erasing text or
polluting RAG chunks. LiteParse's native text geometry is fused with those
regions to rebuild multi-column reading order. Runtime extraction never
invokes `lit`, Python, PaddleOCR, an OCR HTTP service, or a background inference
server.

## Model installation and runtime

Extraction never downloads models. Before processing papers, open
**liteparse-rag: Download OCR, layout, and formula models** from the global
plugin tools menu to download the text detector/recognizer, layout detector,
and formula recognizer into:

```text
<library>/.localref/liteparse-rag/models/
```

The downloads are pinned to the official
[`PP-OCRv6 medium detector`](https://huggingface.co/PaddlePaddle/PP-OCRv6_medium_det_onnx)
and
[`PP-OCRv6 medium recognizer`](https://huggingface.co/PaddlePaddle/PP-OCRv6_medium_rec_onnx)
commits. Each `inference.onnx`, `inference.yml`, and `inference.json` file is
SHA-256 checked and recorded in `model-lock.json`; later runs verify the cache
before it is used. PP-FormulaNet-S uses the official Paddle tokenizer config
and a [commit-pinned ONNX conversion](https://huggingface.co/x3zvawq/paddleocr-js-onnx/tree/51c2133b5a7ea27b795fa8c400fdbfbd5337dd6a/PP_FormulaNet_S)
that is checked against the official Paddle model card sample before its
SHA-256 is accepted. PP-DocLayout-L uses the commit-pinned
[ONNX conversion](https://huggingface.co/x3zvawq/paddleocr-js-onnx/tree/51c2133b5a7ea27b795fa8c400fdbfbd5337dd6a/pp_doclayout_l)
and the official 23-label configuration. PP-OCRv6 uses DirectML first and
falls back to CPU. PP-DocLayout-L and PP-FormulaNet-S use CPU for predictable
cross-machine results. The packaged Windows bundle includes `DirectML.dll`
beside the plugin executable.

The download window accepts an optional proxy address and port. The desktop
status bar and daemon log report each model file, downloaded bytes, total size,
and percentage at 10% intervals. Extraction reports the current item, parsing
and rendering stages, current page/total pages, accumulated formula count, and
artifact-writing stage. Per-page status is live; the daemon log records the
first, every fifth, and final page to avoid flooding long-running logs. Each
plugin page also shows its terminal action result. If an ONNX asset is missing
or fails verification, extraction reports an explicit error with the path and
does not open a network connection. Failures remain in the item's
`liteparse_rag.error` extra and are requeued by the cron worker.

## Workflow and output

Import and file-added hooks only enqueue affected items. A one-minute cron job
processes the queue only after the user has installed the models. The two UI
actions run selected or active PDF items immediately. Select one or more rows
in Localref, then open **Extract selected papers for RAG** to review the count
and submit a single batch; the active-item action remains available for a
one-paper shortcut. Artifacts are published only after a successful temporary
build:

```text
All/<item>/llm/liteparse-rag/
  document.md       # semantic, multi-column Markdown with in-place LaTeX
  layout.json       # geometry, accepted formulas, rejected boxes/reasons, assets
  chunks.jsonl      # external RAG import records
  images/           # embedded figures extracted from the PDF
  pages/            # rendered page screenshots
  regions/          # complete chart/image crops selected by PP-DocLayout-L
  manifest.json     # source checksum, versions, provider, output inventory
```

`liteparse_rag.status`, `source_sha256`, `artifact_dir`, `skip_reason`, and
`error` are stored as Localref item extras. Items with no main file are marked
`skipped` with `skip_reason = "no_main_file"`; they are not failures and are
not placed back on the retry queue. A later file-added hook can enqueue the
item again after a main file becomes available.

The batch result `unchanged` means the current PDF SHA-256, pipeline revision,
OCR model revision, formula engine, and layout engine match the existing
`manifest.json`, and all required document, layout, chunk, image, page, and
region outputs still exist. Such an artifact is already up to date and is not
regenerated.

## Scope

Version 1 accepts PDF primary files. PP-OCRv6 medium recognizes ordinary text;
PP-DocLayout-L separates titles, body text, tables, charts, figures,
references, headers/footers, and formulas. Header/footer/chart text is kept out
of RAG prose, chart/image regions are preserved as crops, and only semantic
formula boxes reach PP-FormulaNet-S. Formula crops use a small guard band so
neighboring prose is not pulled into the recognizer. `manifest.json` reports
formula candidate, accepted, and rejected counts plus rejection reasons;
`layout.json` keeps rejected formula geometry without storing rejected noisy
LaTeX. Tables retain spatially ordered native
text but are not reconstructed into cell-level HTML or Markdown grids. The
plugin does not add a Localref vector index, PaddleOCR-VL, NCNN, Python, or a
service process.
