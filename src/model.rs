use crate::Options;
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs::File,
    io::{BufReader, Read},
    path::{Path, PathBuf},
    process::Command,
};
use walkdir::WalkDir;

pub fn discover(home: &Path) -> Vec<PathBuf> {
    let mut models: Vec<_> = WalkDir::new(home.join("models"))
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .map(|e| e.into_path())
        .filter(|p| {
            let name = p
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_lowercase();
            name.ends_with(".gguf")
                && !name.contains("mmproj")
                && !name.contains("mtp")
                && !p.with_extension("gguf.aria2").exists()
                && p.metadata().is_ok_and(|m| m.len() >= 100 * 1024 * 1024)
        })
        .collect();
    models.sort();
    models
}
pub fn resolve(home: &Path, target: &str) -> Result<PathBuf> {
    let target = target.trim_matches(['"', '\'']);
    let path = Path::new(target);
    if path.is_file() {
        ensure!(
            path.extension()
                .is_some_and(|s| s.eq_ignore_ascii_case("gguf"))
                && !path.with_extension("gguf.aria2").exists(),
            "Choose a completed GGUF file"
        );
        return Ok(dunce::canonicalize(path)?);
    }
    let mut models = discover(home);
    let needle = match target.to_lowercase().as_str() {
        "auto" => String::new(),
        "qwen3" | "qwen3.8" | "3.8" | "qwen27b" => "27b".into(),
        "coder" | "qwen7b" | "qwen2.5" | "qwen2.5-coder" => "7b".into(),
        n => n.to_owned(),
    };
    models.sort_by_key(|p| {
        let n = p
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_lowercase();
        if n == "qwen3.8-27b-ud-q4_k_xl.gguf" {
            0
        } else if n == "qwen2.5-coder-7b-instruct-q4_k_m.gguf" {
            1
        } else {
            2
        }
    });
    models
        .into_iter()
        .find(|p| {
            p.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_lowercase()
                .contains(&needle)
        })
        .with_context(|| {
            format!("Model '{target}' unavailable. Download a GGUF or pass --model <path>.")
        })
}

// GGUF metadata is little endian. Skip vocabulary arrays without allocating their contents.
type Header = BufReader<File>;
fn bytes<const N: usize>(r: &mut Header) -> Result<[u8; N]> {
    let mut b = [0; N];
    r.read_exact(&mut b)?;
    Ok(b)
}
fn word(r: &mut Header) -> Result<u32> {
    Ok(u32::from_le_bytes(bytes(r)?))
}
fn length(r: &mut Header) -> Result<u64> {
    Ok(u64::from_le_bytes(bytes(r)?))
}
fn string(r: &mut Header) -> Result<String> {
    let n = length(r)?;
    ensure!(n <= 16 << 20, "GGUF metadata string too large");
    let mut b = vec![0; n as usize];
    r.read_exact(&mut b)?;
    Ok(String::from_utf8_lossy(&b).into_owned())
}
fn value(r: &mut Header, kind: u32) -> Result<Value> {
    Ok(match kind {
        0 => json!(u8::from_le_bytes(bytes(r)?)),
        1 => json!(i8::from_le_bytes(bytes(r)?)),
        2 => json!(u16::from_le_bytes(bytes(r)?)),
        3 => json!(i16::from_le_bytes(bytes(r)?)),
        4 => json!(word(r)?),
        5 => json!(i32::from_le_bytes(bytes(r)?)),
        6 => json!(f32::from_le_bytes(bytes(r)?)),
        7 => json!(bytes::<1>(r)?[0] != 0),
        8 => json!(string(r)?),
        10 => json!(length(r)?),
        11 => json!(i64::from_le_bytes(bytes(r)?)),
        12 => json!(f64::from_le_bytes(bytes(r)?)),
        9 => {
            let elem = word(r)?;
            let n = length(r)?;
            ensure!(n < 100_000_000, "GGUF array too large");
            if elem == 8 {
                for _ in 0..n {
                    let size = length(r)?;
                    ensure!(size <= 16 << 20, "invalid GGUF string");
                    r.seek_relative(size as i64)?;
                }
            } else {
                let width = match elem {
                    0 | 1 | 7 => 1,
                    2 | 3 => 2,
                    4..=6 => 4,
                    10..=12 => 8,
                    _ => bail!("invalid GGUF array type"),
                };
                r.seek_relative((n * width) as i64)?;
            }
            Value::Null
        }
        _ => bail!("invalid GGUF metadata type"),
    })
}
fn metadata(path: &Path) -> Result<HashMap<String, Value>> {
    let mut r = BufReader::new(File::open(path)?);
    ensure!(
        bytes::<4>(&mut r)? == *b"GGUF" && (2..=3).contains(&word(&mut r)?),
        "Unsupported GGUF header"
    );
    let _tensors = length(&mut r)?;
    let count = length(&mut r)?;
    ensure!(count <= 100_000, "Invalid GGUF metadata count");
    let mut fields = HashMap::new();
    for _ in 0..count {
        let key = string(&mut r)?;
        let kind = word(&mut r)?;
        let v = value(&mut r, kind)?;
        if !v.is_null() {
            fields.insert(key, v);
        }
    }
    Ok(fields)
}

#[derive(Clone)]
pub struct Model {
    pub path: PathBuf,
    pub name: String,
    pub ctx: usize,
    pub reasoning: bool,
    pub spec: String,
    pub threads: Option<usize>,
    pub resident: bool,
    pub temp: f64,
    pub top_p: f64,
    pub top_k: u64,
}
impl Model {
    pub fn inspect(path: PathBuf, opt: &Options) -> Result<Self> {
        let m = metadata(&path).with_context(|| format!("Cannot read {}", path.display()))?;
        let get = |key: &str| m.get(key).unwrap_or(&Value::Null);
        let stem = path.file_stem().unwrap_or_default().to_string_lossy();
        let name = get("general.name").as_str().unwrap_or(&stem).to_owned();
        let arch = get("general.architecture").as_str().unwrap_or("");
        let reasoning = get("tokenizer.chat_template").as_str().map_or_else(
            || {
                ["qwen3", "thinking", "reasoning", "deepseek-r1"]
                    .iter()
                    .any(|s| stem.to_lowercase().contains(s))
            },
            |t| t.contains("<think>") || t.contains("enable_thinking"),
        );
        let size = path.metadata()?.len();
        let smi = std::env::var_os("SystemRoot")
            .map(|root| PathBuf::from(root).join("System32/nvidia-smi.exe"))
            .filter(|p| p.is_file())
            .or_else(|| {
                std::env::var_os("ProgramFiles")
                    .map(|root| PathBuf::from(root).join("NVIDIA Corporation/NVSMI/nvidia-smi.exe"))
                    .filter(|p| p.is_file())
            });
        let vram = smi
            .and_then(|exe| {
                Command::new(exe)
                    .args(["--query-gpu=memory.total", "--format=csv,noheader,nounits"])
                    .output()
                    .ok()
            })
            .and_then(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .filter_map(|s| s.trim().parse::<u64>().ok())
                    .max()
            })
            .unwrap_or(0);
        let resident = size <= vram * 1024 * 1024 * 4 / 5;
        let trained = get(&format!("{arch}.context_length"))
            .as_u64()
            .filter(|n| *n > 0)
            .unwrap_or(u64::MAX);
        let ctx = (opt
            .context_length
            .unwrap_or(if resident { 8192 } else { 16384 }) as usize)
            .min(trained as usize);
        let mtp = get(&format!("{arch}.nextn_predict_layers"))
            .as_u64()
            .unwrap_or(0)
            > 0
            || opt.draft_model.is_some();
        let spec = match opt.speculative.as_str() {
            "auto" if mtp => "draft-mtp",
            "auto" => "none",
            "draft-mtp" if !mtp => bail!("This model needs an MTP draft file"),
            s => s,
        }
        .into();
        let threads = opt.cpu_threads.or_else(|| {
            (!resident).then(|| {
                (std::thread::available_parallelism().map_or(6, usize::from) * 2 / 3).max(1)
            })
        });
        Ok(Self {
            path,
            name,
            ctx,
            reasoning,
            spec,
            threads,
            resident,
            temp: get("general.sampling.temp")
                .as_f64()
                .unwrap_or(if reasoning { 1.0 } else { 0.2 }),
            top_p: get("general.sampling.top_p")
                .as_f64()
                .unwrap_or(if reasoning { 0.95 } else { 0.9 }),
            top_k: get("general.sampling.top_k")
                .as_u64()
                .unwrap_or(if reasoning { 20 } else { 40 }),
        })
    }
}
