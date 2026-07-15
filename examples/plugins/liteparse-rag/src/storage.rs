//! Queue persistence, atomic artifact publication, and file utilities.

use crate::*;

#[derive(Default, Serialize, Deserialize)]
pub(crate) struct Queue {
    items: Vec<String>,
}
impl Queue {
    fn path(root: &Path) -> PathBuf {
        root.join(".localref").join(PLUGIN).join("queue.json")
    }
    pub(crate) fn enqueue(root: &Path, id: &str) -> Result<(), String> {
        let path = Self::path(root);
        with_file_lock(&path, || {
            let mut queue: Self = read_json(&path).unwrap_or_default();
            if !queue.items.iter().any(|item| item == id) {
                queue.items.push(id.to_string());
                write_json(&path, &queue)?;
            }
            Ok(())
        })
    }
    pub(crate) fn take(root: &Path) -> Result<Vec<String>, String> {
        let path = Self::path(root);
        with_file_lock(&path, || {
            let queue: Self = read_json(&path).unwrap_or_default();
            write_json(&path, &Self::default())?;
            Ok(queue.items)
        })
    }
}

pub(crate) fn replace_directory(
    temporary: &Path,
    target: &Path,
) -> Result<(), String> {
    let backup =
        target.with_file_name(format!(".liteparse-rag-previous-{}", nonce()));
    if target.exists() {
        fs::rename(target, &backup)
            .map_err(io("move previous artifacts aside"))?;
    }
    if let Err(error) = fs::rename(temporary, target) {
        if backup.exists() {
            let _ = fs::rename(&backup, target);
        }
        return Err(io("publish artifacts")(error));
    }
    if backup.exists() {
        fs::remove_dir_all(backup).map_err(io("remove previous artifacts"))?;
    }
    Ok(())
}
pub(crate) fn manifest_matches(path: &Path, source_sha: &str) -> bool {
    let Some(artifact) = path.parent() else {
        return false;
    };
    if ["manifest.json", "document.md", "layout.json", "chunks.jsonl"]
        .iter()
        .any(|name| !artifact.join(name).is_file())
        || ["images", "pages", "regions"]
            .iter()
            .any(|name| !artifact.join(name).is_dir())
    {
        return false;
    }
    read_json::<Value>(path).ok().is_some_and(|value| {
        value.get("source_sha256").and_then(Value::as_str) == Some(source_sha)
            && value.get("pipeline_revision").and_then(Value::as_str)
                == Some(PIPELINE_REVISION)
            && value.pointer("/ocr/revision").and_then(Value::as_str)
                == Some(MODEL_REVISION)
            && value
                .pointer("/formula_recognition/engine")
                .and_then(Value::as_str)
                == Some("PP-FormulaNet-S-onnx")
            && value.pointer("/layout_analysis/engine").and_then(Value::as_str)
                == Some("PP-DocLayout-L-onnx")
    })
}
pub(crate) fn read_json<T: for<'a> Deserialize<'a>>(
    path: &Path,
) -> Result<T, String> {
    serde_json::from_slice(&fs::read(path).map_err(io("read JSON"))?)
        .map_err(|e| format!("parse {}: {e}", path.display()))
}
pub(crate) fn write_json<T: Serialize>(
    path: &Path,
    value: &T,
) -> Result<(), String> {
    let parent = path.parent().ok_or("JSON path has no parent")?;
    fs::create_dir_all(parent).map_err(io("create JSON parent"))?;
    let temporary = path.with_extension("tmp");
    fs::write(
        &temporary,
        serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?,
    )
    .map_err(io("write JSON"))?;
    fs::rename(&temporary, path).map_err(io("publish JSON"))
}
pub(crate) fn with_file_lock<T>(
    path: &Path,
    action: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let parent = path.parent().ok_or("lock path has no parent")?;
    fs::create_dir_all(parent).map_err(io("create lock directory"))?;
    let lock = path.with_extension("lock");
    for _ in 0..20 {
        if fs::create_dir(&lock).is_ok() {
            let result = action();
            let _ = fs::remove_dir(&lock);
            return result;
        }
        thread::sleep(Duration::from_millis(50));
    }
    Err(format!("queue is busy: {}", path.display()))
}
pub(crate) fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file =
        fs::File::open(path).map_err(io("open file for SHA-256"))?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read =
            file.read(&mut buffer).map_err(io("read file for SHA-256"))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}
#[cfg(test)]
pub(crate) fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub(crate) fn unix_ms() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis())
}
pub(crate) fn nonce() -> u128 {
    unix_ms()
}
pub(crate) fn io(
    operation: &'static str,
) -> impl FnOnce(std::io::Error) -> String {
    move |error| format!("could not {operation}: {error}")
}
