# LeanVision-main resource mining report

## Scope and files inspected

Resource path: `resources/LeanVision-main/LeanVision-main`.

Inventory: 4 files, about 364 KB:

- `README.md` and `lean4_extractor.py` read in full.
- `input.pdf` and `input.jpg` catalogued as sample OCR inputs.

Generated/bulk artifacts:

- No generated outputs are checked in.
- The PDF/JPG are example inputs and may contain scanned/visual Lean snippets.

## Core idea

LeanVision is a small OCR preprocessor. It calls Mistral OCR on a PDF or image, receives markdown, and optionally extracts fenced Lean code blocks into a `.lean` file.

## Reusable architecture/code patterns for Theoremata

- Input-type dispatch: PDF upload with signed URL vs base64 image data URL.
- Temporary remote upload cleanup for PDFs.
- Markdown-to-Lean extraction via code fences.
- Simple CLI contract: `input_file output_file --api-key --model`.

## Benchmark/eval value

Low-to-medium. It is not a prover, verifier, or benchmark, but it is useful for ingestion. Theoremata may eventually need to ingest screenshots, scanned Lean examples, lecture notes, or PDF problem statements with embedded code. LeanVision is a minimal prototype of that front door.

## Gaps and risks

- `lean4_extractor.py` uses `json.loads(...)` in a fallback path but never imports `json`.
- If no fenced Lean code block is found, it writes raw markdown to `.lean`, which is unsafe.
- No OCR confidence, provenance map, or source-location preservation.
- No post-OCR Lean compile/diagnostic validation.
- Network/API dependency and privacy concerns for uploaded PDFs/images.
- MIME generation emits `image/jpg` for `.jpg`; `image/jpeg` is safer.
- Regex only captures fenced blocks labelled `lean`/`lean4` or unlabeled; it is not a robust markdown parser.

## Concrete integration recommendations

1. Treat OCR as an optional preprocessing stage, never as trusted formal input.
2. Preserve both raw OCR markdown and extracted `.lean` with page/image provenance.
3. Add immediate Lean diagnostics and reject `.lean` outputs that are raw markdown or do not parse.
4. Fix the missing `json` import and MIME mapping before reuse.
5. Use this to create OCR-ingestion fixtures: image/PDF → markdown → Lean candidate → compile diagnostics.
