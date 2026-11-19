use crate::{Mode, safe};
use anyhow::{Context, Result, bail, ensure};
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use serde_json::Value;
use std::{
    fs,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};
const MAX: u64 = 2 * 1024 * 1024;

pub fn string<'a>(v: &'a Value, k: &str) -> Result<&'a str> {
    v[k].as_str()
        .with_context(|| format!("Expected text for {k}"))
}
fn integer(v: &Value, k: &str, default: usize, min: usize, max: usize) -> Result<usize> {
    let n = if v[k].is_null() {
        default
    } else {
        v[k].as_u64()
            .context("Expected a positive integer")?
            .try_into()?
    };
    Ok(n.clamp(min, max))
}
fn excluded(name: &str) -> bool {
    let n = name.to_lowercase();
    let suffix = n.rsplit('.').next().unwrap_or("");
    matches!(
        n.as_str(),
        ".git"
            | ".hg"
            | ".svn"
            | ".venv"
            | "venv"
            | "node_modules"
            | "__pycache__"
            | ".mypy_cache"
            | ".pytest_cache"
            | ".ruff_cache"
            | ".next"
            | ".nuxt"
            | "target"
            | "dist"
            | "build"
            | ".idea"
            | ".vscode"
            | ".local_assistant"
            | ".ssh"
            | "id_rsa"
            | "id_ed25519"
            | "known_hosts"
    ) || n.starts_with(".env")
        || ["password", "secret", "credential"]
            .iter()
            .any(|word| n.contains(word))
        || matches!(
            suffix,
            "pem"
                | "key"
                | "pfx"
                | "p12"
                | "keystore"
                | "gguf"
                | "safetensors"
                | "pt"
                | "pth"
                | "bin"
                | "onnx"
                | "sqlite"
                | "sqlite3"
                | "db"
                | "dll"
                | "exe"
                | "pyd"
                | "pyc"
                | "zip"
                | "7z"
                | "pdf"
                | "png"
                | "jpg"
                | "jpeg"
                | "webp"
                | "mp4"
                | "mp3"
                | "wav"
        )
}
fn read(path: &Path) -> Result<Vec<u8>> {
    let f = fs::File::open(path)?;
    let m = f.metadata()?;
    ensure!(
        m.is_file() && m.len() <= MAX,
        "Expected a regular file of at most 2 MiB"
    );
    let mut b = Vec::new();
    f.take(MAX + 1).read_to_end(&mut b)?;
    ensure!(
        b.len() as u64 <= MAX && !b.contains(&0),
        "File is too large or binary"
    );
    Ok(b)
}
fn text(b: &[u8]) -> Result<&str> {
    Ok(std::str::from_utf8(
        b.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(b),
    )?)
}
fn normalize(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n")
}

pub struct Workspace {
    pub root: PathBuf,
    ignore: Gitignore,
}
impl Workspace {
    pub fn open(path: &Path) -> Result<Self> {
        let root = dunce::canonicalize(path).context("Workspace folder unavailable")?;
        ensure!(root.is_dir(), "Expected a workspace directory");
        let mut builder = GitignoreBuilder::new(&root);
        let ignorefile = root.join(".gitignore");
        if fs::symlink_metadata(&ignorefile)
            .is_ok_and(|m| m.is_file() && !crate::linked(&m) && m.len() <= 100_000)
        {
            builder.add(ignorefile);
        }
        Ok(Self {
            root,
            ignore: builder.build()?,
        })
    }
    pub fn path(&self, s: &str) -> Result<PathBuf> {
        let request = Path::new(s.trim_matches(['"', '\'']));
        let rel = if request.is_absolute() {
            request
                .strip_prefix(&self.root)
                .context("Path leaves the workspace")?
        } else {
            request
        };
        let mut dest = self.root.clone();
        for part in rel.components() {
            match part {
                Component::CurDir => continue,
                Component::ParentDir => {
                    ensure!(dest != self.root, "Path leaves workspace");
                    dest.pop();
                }
                Component::Normal(p) => {
                    let name = p.to_string_lossy();
                    let lower = name.to_ascii_lowercase();
                    let base = lower.split('.').next().unwrap_or("").trim_end();
                    ensure!(
                        !excluded(&name)
                            && !name.ends_with(['.', ' '])
                            && !name.contains([':', '\0'])
                            && !matches!(
                                base,
                                "nul" | "con" | "prn" | "aux" | "conin$" | "conout$"
                            )
                            && !(base.starts_with("com") || base.starts_with("lpt"))
                                .then(|| base.chars().skip(3).collect::<String>())
                                .is_some_and(|d| matches!(
                                    d.as_str(),
                                    "1" | "2"
                                        | "3"
                                        | "4"
                                        | "5"
                                        | "6"
                                        | "7"
                                        | "8"
                                        | "9"
                                        | "¹"
                                        | "²"
                                        | "³"
                                )),
                        "Path is reserved or excluded"
                    );
                    dest.push(p);
                    match fs::symlink_metadata(&dest) {
                        Ok(m) => {
                            ensure!(!crate::linked(&m), "Links and junctions are excluded");
                            ensure!(
                                !self
                                    .ignore
                                    .matched_path_or_any_parents(&dest, m.is_dir())
                                    .is_ignore(),
                                "Git-ignored path"
                            );
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                            ensure!(
                                !self
                                    .ignore
                                    .matched_path_or_any_parents(&dest, false)
                                    .is_ignore(),
                                "Git-ignored path"
                            );
                        }
                        Err(e) => return Err(e.into()),
                    }
                }
                _ => bail!("Path leaves the workspace"),
            }
        }
        Ok(dest)
    }
    fn walk<'a>(
        &'a self,
        base: &Path,
        depth: usize,
    ) -> impl Iterator<Item = walkdir::DirEntry> + 'a {
        walkdir::WalkDir::new(base)
            .max_depth(depth + 1)
            .follow_links(false)
            .into_iter()
            .filter_entry(move |e| {
                e.depth() == 0
                    || (!excluded(&e.file_name().to_string_lossy())
                        && fs::symlink_metadata(e.path()).is_ok_and(|m| !crate::linked(&m))
                        && !self
                            .ignore
                            .matched(e.path(), e.file_type().is_dir())
                            .is_ignore())
            })
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_file())
    }
    pub fn guidance(&self) -> String {
        self.path("AGENTS.md")
            .ok()
            .and_then(|p| {
                p.metadata()
                    .ok()
                    .filter(|m| m.len() <= 12_000)
                    .and_then(|_| read(&p).ok())
            })
            .and_then(|b| text(&b).ok().map(str::to_owned))
            .unwrap_or_default()
    }
    pub fn dispatch(&self, name: &str, args: &Value, mode: Mode) -> Result<String> {
        if mode == Mode::Plan && !matches!(name, "list_files" | "read_file" | "search_text") {
            bail!("Plan mode permits only workspace inspection");
        }
        let s = args["path"].as_str().unwrap_or(".");
        match name {
            "list_files" => {
                let dir = self.path(s)?;
                ensure!(dir.is_dir(), "Expected a folder");
                let mut entries = Vec::new();
                for e in self
                    .walk(&dir, integer(args, "max_depth", 4, 1, 6)?)
                    .take(integer(args, "max_entries", 120, 1, 200)?)
                {
                    entries.push(format!(
                        "{} ({} bytes)",
                        e.path().strip_prefix(&self.root)?.display(),
                        e.metadata()?.len()
                    ));
                }
                Ok(if entries.is_empty() {
                    "No readable files found.".into()
                } else {
                    entries.join("\n")
                })
            }
            "read_file" => {
                let data = read(&self.path(string(args, "path")?)?)?;
                let start = integer(args, "start_line", 1, 1, usize::MAX)?;
                let end = integer(
                    args,
                    "end_line",
                    start.saturating_add(399),
                    start,
                    start.saturating_add(399),
                )?;
                let lines = text(&data)?
                    .lines()
                    .enumerate()
                    .skip(start - 1)
                    .take(end - start + 1)
                    .map(|(i, l)| format!("{}: {l}", i + 1))
                    .collect::<Vec<_>>();
                Ok(format!("File: {s}\n{}", lines.join("\n")))
            }
            "search_text" => {
                let q = string(args, "query")?.to_lowercase();
                ensure!(
                    !q.trim().is_empty() && q.chars().count() <= 500,
                    "Invalid query"
                );
                let glob = globset::Glob::new(args["file_glob"].as_str().unwrap_or("*"))?
                    .compile_matcher();
                let cap = integer(args, "max_results", 30, 1, 50)?;
                let base = self.path(s)?;
                ensure!(base.exists(), "Path does not exist");
                let mut hits = Vec::new();
                let mut scanned = 0;
                for e in self
                    .walk(&base, 10)
                    .filter(|e| glob.is_match(e.file_name()))
                    .take(4000)
                {
                    if crate::interrupted() {
                        bail!("Interrupted")
                    }
                    scanned += 1;
                    let Ok(b) = read(e.path()) else { continue };
                    let Ok(src) = text(&b) else { continue };
                    for (i, l) in src.lines().enumerate() {
                        if l.to_lowercase().contains(&q) {
                            hits.push(format!(
                                "{}:{}: {}",
                                e.path().strip_prefix(&self.root)?.display(),
                                i + 1,
                                crate::clip(l.trim(), 300)
                            ));
                            if hits.len() == cap {
                                return Ok(format!("{}\n[results capped]", hits.join("\n")));
                            }
                        }
                    }
                }
                Ok(if hits.is_empty() {
                    format!("No matches in {scanned} files.")
                } else {
                    hits.join("\n")
                })
            }
            "write_file" | "replace_text" | "delete_file" => self.edit(name, args, mode),
            "run_command" => {
                let cmd = string(args, "command")?;
                ensure!(
                    !cmd.trim().is_empty() && cmd.chars().count() <= 5000,
                    "Invalid command"
                );
                println!("\nPowerShell in {}\n{}", self.root.display(), safe(cmd));
                if mode != Mode::Full && !crate::ui::confirm("Run this command?")? {
                    bail!("User declined the command");
                }
                crate::process::shell(
                    &self.root,
                    cmd,
                    integer(args, "timeout_seconds", 120, 1, 300)? as u64,
                )
            }
            _ => bail!("Unknown workspace tool: {name}"),
        }
    }
    fn edit(&self, name: &str, args: &Value, mode: Mode) -> Result<String> {
        let rel = string(args, "path")?;
        let path = self.path(rel)?;
        ensure!(path != self.root, "Expected a file");
        let before = if path.exists() {
            Some(read(&path)?)
        } else {
            None
        };
        let old = text(before.as_deref().unwrap_or_default())?;
        let mut next = match name {
            "delete_file" => {
                ensure!(before.is_some(), "File does not exist");
                String::new()
            }
            "replace_text" => {
                ensure!(before.is_some(), "File does not exist");
                let from = normalize(string(args, "old_text")?);
                let to = string(args, "new_text")?;
                ensure!(
                    !from.is_empty()
                        && from.chars().count() <= 150_000
                        && to.chars().count() <= 150_000,
                    "Invalid replacement size"
                );
                let normalized = normalize(old);
                ensure!(
                    normalized.matches(&from).count() == 1,
                    "old_text must match exactly once"
                );
                normalized.replacen(&from, &normalize(to), 1)
            }
            _ => {
                let s = string(args, "content")?;
                ensure!(
                    s.chars().count() <= 150_000,
                    "Write exceeds 150,000 characters"
                );
                normalize(s)
            }
        };
        if old.contains("\r\n") {
            next = next.replace('\n', "\r\n");
        }
        if name != "delete_file" && before.is_some() && next == old {
            return Ok("No change needed.".into());
        }
        let diff = similar::TextDiff::from_lines(old, &next)
            .unified_diff()
            .header(
                rel,
                if name == "delete_file" {
                    "/dev/null"
                } else {
                    rel
                },
            )
            .to_string();
        ensure!(
            diff.len() <= 50_000,
            "Diff too large to review. Use a smaller change."
        );
        println!("\n{}  {}\n{}", name, safe(rel), safe(&diff));
        if mode == Mode::Ask && !crate::ui::confirm("Apply this change?")? {
            bail!("User declined the change");
        }
        self.path(rel)?;
        ensure!(
            match &before {
                Some(bytes) => read(&path).is_ok_and(|b| b == *bytes),
                None => !path.exists(),
            },
            "File changed during review. Read it again."
        );
        if name == "delete_file" {
            fs::remove_file(path)?;
            return Ok(format!("Deleted {rel}"));
        }
        let parent = path.parent().context("Invalid target")?;
        fs::create_dir_all(parent)?;
        self.path(rel)?;
        let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
        if before
            .as_ref()
            .is_some_and(|b| b.starts_with(&[0xef, 0xbb, 0xbf]))
        {
            tmp.write_all(&[0xef, 0xbb, 0xbf])?;
        }
        tmp.write_all(next.as_bytes())?;
        tmp.flush()?;
        if before.is_none() {
            tmp.persist_noclobber(&path)?;
        } else {
            tmp.persist(&path)?;
        }
        Ok(format!("Wrote {rel}"))
    }
}
