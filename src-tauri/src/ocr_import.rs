use anyhow::{anyhow, Result};
use serde::Serialize;
use serde_json::Value;
use std::io::Write;
use std::process::Command;

use crate::module::ModuleDef;

/// Runs Tesseract OCR on image bytes and returns the raw extracted text.
/// Shells out to the `tesseract` CLI rather than using a Rust binding
/// crate — simpler, more robust, and avoids pulling in another large
/// native-dependency tree for one feature. Requires `tesseract-ocr`
/// installed on the host (apt/brew/choco all package it).
pub fn extract_text(image_bytes: &[u8]) -> Result<String> {
    let tmp = tempfile_path();
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(image_bytes)?;
    }

    let output = Command::new("tesseract")
        .arg(&tmp)
        .arg("stdout")
        .output();

    let _ = std::fs::remove_file(&tmp); // best-effort cleanup, don't fail the request over it

    let output = output.map_err(|e| {
        anyhow!("could not run `tesseract` ({e}) — is tesseract-ocr installed on this machine?")
    })?;

    if !output.status.success() {
        return Err(anyhow!(
            "tesseract exited with an error: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn tempfile_path() -> std::path::PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!("ocr_import_{}.png", uuid::Uuid::new_v4()));
    p
}

#[derive(Debug, Serialize)]
pub struct CandidateRecord {
    pub raw_line: String,
    pub fields: serde_json::Map<String, Value>,
    /// Field names the parser genuinely recognized (a number token where
    /// a numeric field was expected). Anything not in this list was
    /// filled in best-effort by position and should be double-checked —
    /// this is what the "review before import" step in the UI is for.
    pub confident_fields: Vec<String>,
    /// Required fields the source document simply didn't contain (most
    /// commonly a SKU/code that only exists in a formal inventory
    /// system, never on a handwritten ledger). These candidates will be
    /// rejected by bulk-create until a human fills these in — this list
    /// is what lets the review UI ask for exactly the right thing
    /// instead of the import silently failing.
    pub missing_required_fields: Vec<String>,
}

/// Heuristically maps raw OCR text into candidate records for a given
/// module. Deliberately conservative: this NEVER auto-creates records —
/// it only proposes them, tagged with which fields it's actually
/// confident about, for a human to review. OCR on real handwritten
/// ledgers is messy (see the honest example in the docs); pretending
/// otherwise would be worse than a small amount of manual cleanup.
pub fn parse_into_candidates(module: &ModuleDef, raw_text: &str) -> Vec<CandidateRecord> {
    let numeric_fields: Vec<&str> = module.fields.iter()
        .filter(|f| f.field_type == "integer" || f.field_type == "real")
        .map(|f| f.name.as_str())
        .collect();
    let text_fields: Vec<&str> = module.fields.iter()
        .filter(|f| f.field_type == "text")
        .map(|f| f.name.as_str())
        .collect();

    let mut candidates = Vec::new();

    for line in raw_text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        // Tesseract tends to preserve column-like spacing as runs of
        // whitespace — split on 2+ spaces/tabs first (closer to real
        // columns), falling back to single-space tokens if that yields
        // only one chunk (a line with no wide gaps at all).
        let wide_gap_split: Vec<&str> = line.split("  ").map(str::trim).filter(|s| !s.is_empty()).collect();
        let tokens: Vec<&str> = if wide_gap_split.len() > 1 {
            wide_gap_split
        } else {
            line.split_whitespace().collect()
        };

        let mut fields = serde_json::Map::new();
        let mut confident_fields = Vec::new();
        let mut numeric_idx = 0;
        let mut text_tokens: Vec<&str> = Vec::new();

        for token in &tokens {
            let cleaned = token.replace(',', "");
            if let Ok(n) = cleaned.parse::<f64>() {
                if numeric_idx < numeric_fields.len() {
                    let field_name = numeric_fields[numeric_idx];
                    let field_type = module.fields.iter().find(|f| f.name == field_name).map(|f| f.field_type.as_str());
                    let value = if field_type == Some("integer") {
                        serde_json::json!(n.round() as i64)
                    } else {
                        serde_json::json!(n)
                    };
                    fields.insert(field_name.to_string(), value);
                    confident_fields.push(field_name.to_string());
                    numeric_idx += 1;
                }
            } else {
                text_tokens.push(token);
            }
        }

        // Whatever wasn't a number goes into the free-text description
        // field. Prefer a field actually named `name`/`description` if
        // the module has one — almost always the right target — rather
        // than blindly using the first text field in schema order (which
        // for a module like inventory would otherwise be `sku`, not
        // `name`, giving a technically-not-wrong but practically
        // unhelpful default).
        let target_text_field = text_fields.iter()
            .find(|f| **f == "name" || **f == "description" || **f == "item_name" || **f == "full_name")
            .or_else(|| text_fields.first());

        if let Some(&target_field) = target_text_field {
            if !text_tokens.is_empty() {
                fields.insert(target_field.to_string(), serde_json::json!(text_tokens.join(" ")));
            }
        }

        if !fields.is_empty() {
            let missing_required: Vec<String> = module.fields.iter()
                .filter(|f| f.required && f.default.is_none() && !fields.contains_key(&f.name))
                .map(|f| f.name.clone())
                .collect();

            candidates.push(CandidateRecord {
                raw_line: line.to_string(),
                fields,
                confident_fields,
                missing_required_fields: missing_required,
            });
        }
    }

    candidates
}
