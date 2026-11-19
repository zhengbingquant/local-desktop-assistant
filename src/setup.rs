use crate::{
    Options,
    model::{self, Model},
    safe,
};
use anyhow::{Context, Result, bail, ensure};
use clap::Subcommand;
use reqwest::blocking::Client;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, BufReader, Read, Seek, SeekFrom, Write},
    path::{Component, Path},
    process::Command,
    time::Duration,
};
use uuid::Uuid;

#[derive(Clone, Subcommand)]
pub enum Setup {
    /// Install the local llama.cpp Windows backend from a GitHub release
    InstallRuntime {
        #[arg(long, default_value = "b11748")]
        tag: String,
        #[arg(long,default_value="cuda-13",value_parser=["cuda-13","cuda-12","vulkan","cpu"])]
        backend: String,
        #[arg(long)]
        force: bool,
        #[arg(long)]
        dry_run: bool,
    },
    /// Download and verify a GGUF. Interrupted transfers resume on the next invocation.
    Download {
        #[arg(value_parser=["27b","7b"])]
        variant: String,
        #[arg(long)]
        vision: bool,
        #[arg(long)]
        mtp: bool,
    },
    /// Install luma and local-assistant commands on the current user's PATH
    InstallCommand,
    /// Start llama.cpp plain offline chat without agent tools or saved sessions
    Chat,
}
impl Setup {
    pub fn run(&self, home: &Path, opt: &Options) -> Result<()> {
        match self {
            Self::InstallRuntime {
                tag,
                backend,
                force,
                dry_run,
            } => runtime(home, tag, backend, *force, *dry_run),
            Self::Download {
                variant,
                vision,
                mtp,
            } => weights(home, variant, *vision, *mtp),
            Self::InstallCommand => install_command(home),
            Self::Chat => plain(home, opt),
        }
    }
}
fn client() -> Result<Client> {
    Ok(Client::builder()
        .https_only(true)
        .user_agent("luma-local-assistant-installer")
        .connect_timeout(Duration::from_secs(20))
        .timeout(Duration::from_secs(24 * 3600))
        .build()?)
}
fn lock(dir: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(dir.join(".install.lock"))?;
    file.try_lock()
        .context("Another install is running in this directory")?;
    Ok(file)
}
fn checksum(path: &Path) -> Result<String> {
    let mut sha = Sha256::new();
    io::copy(&mut BufReader::new(File::open(path)?), &mut sha)?;
    Ok(format!("{:x}", sha.finalize()))
}
fn patterns(backend: &str) -> Vec<String> {
    let suffix = if backend.starts_with("cuda") {
        format!("{backend}*-x64.zip")
    } else {
        format!("{backend}-x64.zip")
    };
    let mut patterns = vec![format!("llama-*-bin-win-{suffix}")];
    if backend.starts_with("cuda") {
        patterns.push(format!("cudart-llama-bin-win-{suffix}"));
    }
    patterns
}
fn runtime(home: &Path, tag: &str, backend: &str, force: bool, dry: bool) -> Result<()> {
    let parent = home.join("runtime");
    let dir = parent.join("llama.cpp");
    if dir.join("llama-server.exe").is_file() && !force && !dry {
        println!(
            "Runtime already installed at {}. Use --force to replace it.",
            dir.display()
        );
        return Ok(());
    }
    ensure!(
        tag.bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-')),
        "Invalid release tag"
    );
    let api = "https://api.github.com/repos/ggml-org/llama.cpp/releases";
    let url = if tag == "latest" {
        format!("{api}?per_page=20")
    } else {
        format!("{api}/tags/{tag}")
    };
    println!("Looking up llama.cpp {tag} ({backend}) on GitHub…");
    let client = client()?;
    let response: Value = client.get(url).send()?.error_for_status()?.json()?;
    let releases = response
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(std::slice::from_ref(&response));
    let filters = patterns(backend)
        .iter()
        .map(|p| globset::Glob::new(p).map(|g| g.compile_matcher()))
        .collect::<Result<Vec<_>, _>>()?;
    let assets = releases
        .iter()
        .find_map(|release| {
            let assets = release["assets"].as_array()?;
            filters
                .iter()
                .map(|g| {
                    assets
                        .iter()
                        .find(|a| a["name"].as_str().is_some_and(|n| g.is_match(n)))
                })
                .collect::<Option<Vec<_>>>()
        })
        .context("No matching release assets")?;
    for a in &assets {
        println!(
            "  {} · {} MiB",
            a["name"],
            a["size"].as_u64().unwrap_or(0) >> 20
        );
    }
    if dry {
        return Ok(());
    }
    fs::create_dir_all(&parent)?;
    let _lock = lock(&parent)?;
    ensure!(
        force || !dir.exists(),
        "Existing runtime directory. Use --force to replace it."
    );
    let tmp = tempfile::tempdir_in(&parent)?;
    let staged = tmp.path().join("install");
    fs::create_dir(&staged)?;
    for (index, a) in assets.iter().enumerate() {
        let expected = a["digest"]
            .as_str()
            .and_then(|s| s.strip_prefix("sha256:"))
            .context("GitHub did not supply a SHA-256 digest")?;
        let archive = tmp.path().join("download.zip");
        let mut file = File::create(&archive)?;
        client
            .get(
                a["browser_download_url"]
                    .as_str()
                    .context("Missing asset URL")?,
            )
            .send()?
            .error_for_status()?
            .copy_to(&mut file)?;
        drop(file);
        ensure!(
            checksum(&archive)?.eq_ignore_ascii_case(expected),
            "Archive checksum mismatch"
        );
        let mut zip = zip::ZipArchive::new(BufReader::new(File::open(&archive)?))?;
        let prefix = if index == 0 {
            zip.file_names()
                .map(Path::new)
                .find(|p| p.file_name().is_some_and(|n| n == "llama-server.exe"))
                .context("Archive is missing llama-server.exe")?
                .parent()
                .unwrap_or(Path::new(""))
                .to_owned()
        } else {
            Default::default()
        };
        for i in 0..zip.len() {
            let mut entry = zip.by_index(i)?;
            if !entry.is_file() {
                continue;
            }
            let path = entry.enclosed_name().context("Unsafe archive entry")?;
            ensure!(
                path.components().all(|c| matches!(c, Component::Normal(_)))
                    && !path.to_string_lossy().contains(':'),
                "Invalid archive path"
            );
            let name = path.file_name().context("Invalid archive filename")?;
            let rel = if path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("dll"))
            {
                Path::new(name)
            } else {
                path.strip_prefix(&prefix).unwrap_or(&path)
            };
            let target = staged.join(rel);
            fs::create_dir_all(target.parent().unwrap())?;
            let mut out = File::create(target)?;
            io::copy(&mut entry, &mut out)?;
        }
    }
    ensure!(
        staged.join("llama-server.exe").is_file(),
        "Missing staged server"
    );
    let backup = if dir.exists() {
        let mut p = parent.join("llama.cpp.previous");
        if p.exists() {
            p = parent.join(format!("llama.cpp.previous-{}", Uuid::new_v4().simple()));
        }
        fs::rename(&dir, &p)?;
        Some(p)
    } else {
        None
    };
    if let Err(e) = fs::rename(&staged, &dir) {
        if let Some(p) = &backup {
            fs::rename(p, &dir).with_context(|| {
                format!("Install failed ({e}). Prior runtime is at {}", p.display())
            })?;
        }
        return Err(e.into());
    }
    println!("Installed to {}", dir.display());
    if let Some(p) = backup {
        println!("Previous runtime: {}", p.display());
    }
    Ok(())
}
struct Asset {
    file: &'static str,
    bytes: u64,
    hash: &'static str,
}
const LARGE: Asset = Asset {
    file: "Qwen3.8-27B-UD-Q4_K_XL.gguf",
    bytes: 17559178144,
    hash: "3f227079003add2511437e5b1e94812e363385225bf6a9b47b0054a72bc8b01e",
};
const VISION: Asset = Asset {
    file: "mmproj-F16.gguf",
    bytes: 927607488,
    hash: "cbb841a9ee0636b2ec172f5bb8df2ea8dfeb01e90fe7c6126581d662a0b4e43e",
};
const MTP: Asset = Asset {
    file: "MTP/mtp-Qwen3.8-27B-Q4_0.gguf",
    bytes: 1369590656,
    hash: "50d9ce5a6da381bbcfb31061cf73df94a90e6faf8efeddee379a9cb8f1501c6e",
};
const SMALL: Asset = Asset {
    file: "qwen2.5-coder-7b-instruct-q4_k_m.gguf",
    bytes: 4683073536,
    hash: "509287f78cb4d4cf6b3843734733b914b2c158e43e22a7f4bf5e963800894d3c",
};
fn weights(home: &Path, model: &str, vision: bool, mtp: bool) -> Result<()> {
    ensure!(
        model == "27b" || !vision && !mtp,
        "The vision and MTP options belong to the 27B model"
    );
    let (folder, repo, mut assets) = if model == "27b" {
        ("Qwen3.8-27B-HF", "unsloth/Qwen3.8-27B-GGUF", vec![LARGE])
    } else {
        (
            "Qwen2.5-Coder-7B",
            "Qwen/Qwen2.5-Coder-7B-Instruct-GGUF",
            vec![SMALL],
        )
    };
    if vision {
        assets.push(VISION)
    }
    if mtp {
        assets.push(MTP)
    }
    let dir = home.join("models").join(folder);
    fs::create_dir_all(&dir)?;
    let _lock = lock(&dir)?;
    let client = client()?;
    for asset in assets {
        let path = dir.join(asset.file);
        fs::create_dir_all(path.parent().unwrap())?;
        if path.is_file() {
            if path.metadata()?.len() == asset.bytes && checksum(&path)? == asset.hash {
                println!("{} verified.", path.display());
                continue;
            }
            if !crate::ui::confirm(&format!(
                "Replace {} after verifying the new download?",
                path.display()
            ))? {
                bail!("Cancelled");
            }
        }
        let partial = path.with_extension("gguf.part");
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&partial)?;
        let start = file.metadata()?.len();
        if start > asset.bytes {
            file.set_len(0)?;
        }
        let offset = file.seek(SeekFrom::End(0))?;
        println!(
            "Downloading {} from Hugging Face · {} MiB already present",
            asset.file,
            offset >> 20
        );
        if offset < asset.bytes {
            let mut req = client.get(format!(
                "https://huggingface.co/{repo}/resolve/main/{}?download=true",
                asset.file
            ));
            if offset > 0 {
                req = req.header("Range", format!("bytes={offset}-"));
            }
            let mut resp = req.send()?.error_for_status()?;
            if resp.status().as_u16() == 206 {
                ensure!(
                    resp.headers()
                        .get("content-range")
                        .and_then(|h| h.to_str().ok())
                        .is_some_and(|s| s.starts_with(&format!("bytes {offset}-"))),
                    "Server returned a different byte range"
                );
            } else {
                file.set_len(0)?;
                file.seek(SeekFrom::Start(0))?;
            }
            let mut buf = vec![0; 256 * 1024];
            let mut downloaded = file.stream_position()?;
            let mut notice = std::time::Instant::now();
            loop {
                ensure!(
                    !crate::interrupted(),
                    "Download paused. Run the same command to resume."
                );
                let n = resp.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                downloaded += n as u64;
                ensure!(downloaded <= asset.bytes, "Download exceeds expected size");
                file.write_all(&buf[..n])?;
                if notice.elapsed() >= Duration::from_secs(10) {
                    println!(
                        "  {:.1}% · {} MiB",
                        downloaded as f64 * 100. / asset.bytes as f64,
                        downloaded >> 20
                    );
                    notice = std::time::Instant::now();
                }
            }
        }
        file.sync_all()?;
        ensure!(
            file.metadata()?.len() == asset.bytes,
            "Incomplete download; run again to resume"
        );
        drop(file);
        if checksum(&partial)? != asset.hash {
            fs::remove_file(partial)?;
            bail!("Model checksum mismatch. Run the download again.");
        }
        fs::rename(&partial, &path)?;
        let control = path.with_extension("gguf.aria2");
        if control.is_file() {
            fs::remove_file(control)?;
        }
        println!("Verified {}", path.display());
    }
    Ok(())
}
fn plain(home: &Path, opt: &Options) -> Result<()> {
    let model = Model::inspect(model::resolve(home, &opt.model)?, opt)?;
    let exe = home.join("runtime/llama.cpp/llama-completion.exe");
    ensure!(exe.is_file(), "Install llama.cpp with luma install-runtime");
    let mut cmd = Command::new(exe);
    cmd.arg("--model").arg(&model.path).args([
        "--conversation",
        "--interactive-first",
        "--simple-io",
        "--offline",
        "--ctx-size",
        &model.ctx.to_string(),
        "--fit",
        "on",
        "--fit-target",
        &opt.gpu_reserve_mib.to_string(),
        "--flash-attn",
        "on",
        "--cache-type-k",
        &opt.kv_cache_type,
        "--cache-type-v",
        &opt.kv_cache_type,
        "--batch-size",
        if model.resident { "256" } else { "512" },
        "--ubatch-size",
        if model.resident { "128" } else { "512" },
        "--reasoning",
        if model.reasoning { "on" } else { "off" },
        "--temp",
        &model.temp.to_string(),
        "--top-p",
        &model.top_p.to_string(),
        "--top-k",
        &model.top_k.to_string(),
    ]);
    if let Some(n) = model.threads {
        cmd.args(["--threads", &n.to_string()]);
    }
    println!("{} · plain local chat", safe(&model.name));
    ensure!(cmd.status()?.success(), "Plain chat process stopped");
    Ok(())
}
#[cfg(windows)]
fn install_command(home: &Path) -> Result<()> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        HWND_BROADCAST, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_SETTINGCHANGE,
    };
    use winreg::{
        RegKey, RegValue,
        enums::{HKEY_CURRENT_USER, REG_EXPAND_SZ},
    };
    let bin =
        std::path::PathBuf::from(std::env::var_os("LOCALAPPDATA").context("Missing LOCALAPPDATA")?)
            .join("Programs/LocalAssistant/bin");
    fs::create_dir_all(&bin)?;
    let exe = std::env::current_exe()?;
    let launch = format!(
        "\u{feff}& '{}' --home '{}' @args\r\nexit $LASTEXITCODE\r\n",
        exe.display().to_string().replace('\'', "''"),
        home.display().to_string().replace('\'', "''")
    );
    fs::write(bin.join("launch.ps1"), launch)?;
    for name in ["luma.cmd", "local-assistant.cmd"] {
        fs::write(
            bin.join(name),
            "@echo off\r\npowershell.exe -NoProfile -ExecutionPolicy Bypass -File \"%~dp0launch.ps1\" %*\r\nexit /b %ERRORLEVEL%\r\n",
        )?;
    }
    let (env, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey("Environment")?;
    // Read the unexpanded registry data so existing %VARIABLE% entries survive.
    let old = match env.get_raw_value("Path") {
        Ok(v) => String::from_utf16_lossy(
            &v.bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| u16::from_le_bytes(*b))
                .take_while(|n| *n != 0)
                .collect::<Vec<_>>(),
        ),
        Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e.into()),
    };
    if !old
        .split(';')
        .any(|s| crate::same_path(Path::new(s.trim_end_matches('\\')), &bin))
    {
        let path = format!("{};{}", old.trim_end_matches(';'), bin.display())
            .trim_start_matches(';')
            .to_owned();
        let value = path
            .encode_utf16()
            .chain([0])
            .flat_map(u16::to_le_bytes)
            .collect();
        env.set_raw_value(
            "Path",
            &RegValue {
                vtype: REG_EXPAND_SZ,
                bytes: value,
            },
        )?;
        let name: Vec<_> = "Environment".encode_utf16().chain([0]).collect();
        unsafe {
            SendMessageTimeoutW(
                HWND_BROADCAST,
                WM_SETTINGCHANGE,
                0,
                name.as_ptr() as isize,
                SMTO_ABORTIFHUNG,
                2000,
                std::ptr::null_mut(),
            );
        }
    }
    println!("Installed luma and local-assistant. Reopen your terminal to use them.");
    Ok(())
}
#[cfg(not(windows))]
fn install_command(_: &Path) -> Result<()> {
    bail!("The command installer is for Windows. Run the release binary directly.")
}
