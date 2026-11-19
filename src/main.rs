//! Luma, a local-first terminal coding assistant.
mod agent;
mod history;
mod model;
mod process;
mod runtime;
mod sessions;
mod setup;
mod ui;
mod web;
mod workspace;

use anyhow::{Context, Result, bail, ensure};
use clap::Parser;
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

#[derive(Parser, Clone)]
#[command(
    name = "luma",
    version,
    about = "Local coding agent with a slash-command palette",
    after_help = "Inside Luma: / commands · Shift+Tab modes · /resume conversations"
)]
struct Options {
    #[command(subcommand)]
    action: Option<setup::Setup>,
    /// Folder the agent can inspect and edit
    #[arg(global = true, long, default_value = ".")]
    workspace: PathBuf,
    /// Installation directory containing models/ and runtime/
    #[arg(global = true, long, env = "LUMA_HOME")]
    home: Option<PathBuf>,
    /// auto, 27b, 7b, a local filename fragment, or a GGUF path
    #[arg(global = true, long, default_value = "auto")]
    model: String,
    #[arg(global=true, long,value_parser=clap::value_parser!(u32).range(2048..=262144))]
    context_length: Option<u32>,
    #[arg(global = true, long)]
    cpu_threads: Option<usize>,
    #[arg(global = true, long)]
    batch_threads: Option<usize>,
    #[arg(global=true, long,default_value_t=512,value_parser=clap::value_parser!(u32).range(128..=4096))]
    gpu_reserve_mib: u32,
    #[arg(global=true, long,default_value="q8_0",value_parser=["q8_0","f16"])]
    kv_cache_type: String,
    #[arg(global=true, long,default_value="auto",value_parser=["auto","none","draft-mtp","ngram-mod","ngram-simple"])]
    speculative: String,
    #[arg(global = true, long)]
    draft_model: Option<PathBuf>,
    #[arg(global=true, long,default_value="medium",value_parser=["none","low","medium","high"])]
    reasoning_effort: String,
    #[arg(global=true, long,default_value_t=512,value_parser=clap::value_parser!(u32).range(0..=8192))]
    thinking_budget: u32,
    #[arg(global=true, long,default_value="show",value_parser=["show","hide"])]
    thinking_display: String,
    #[arg(global=true, long,default_value="off",value_parser=["off","auto"])]
    web_mode: String,
    #[arg(global = true, long, conflicts_with = "disable_web")]
    enable_web: bool,
    #[arg(global = true, long)]
    disable_web: bool,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Ask,
    Edit,
    Plan,
    Full,
}
impl Mode {
    fn key(self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::Edit => "workspace",
            Self::Plan => "plan",
            Self::Full => "full",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Ask => "Ask",
            Self::Edit => "Accept edits",
            Self::Plan => "Plan",
            Self::Full => "Full access",
        }
    }
    fn next(self) -> Self {
        match self {
            Self::Ask => Self::Edit,
            Self::Edit => Self::Plan,
            Self::Plan | Self::Full => Self::Ask,
        }
    }
    fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "ask" | "default" => Self::Ask,
            "edit" | "edits" | "workspace" | "accept-edits" => Self::Edit,
            "plan" => Self::Plan,
            "full" => Self::Full,
            _ => bail!("Use ask, workspace, plan or full"),
        })
    }
}
pub fn safe(s: &str) -> String {
    let mut output = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_control() && !matches!(c, '\n' | '\t') {
            output.extend(c.escape_default());
        } else {
            output.push(c);
        }
    }
    output
}
pub fn clip(s: &str, n: usize) -> String {
    match s.char_indices().nth(n) {
        Some((idx, _)) => format!("{}…", &s[..idx]),
        None => s.to_owned(),
    }
}
pub fn linked(m: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        m.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        m.file_type().is_symlink()
    }
}
fn same_path(a: &Path, b: &Path) -> bool {
    if cfg!(windows) {
        a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
    } else {
        a == b
    }
}
static INTERRUPTED: AtomicBool = AtomicBool::new(false);
fn interrupted() -> bool {
    INTERRUPTED.load(Ordering::Relaxed)
}
fn reset_interrupt() {
    INTERRUPTED.store(false, Ordering::Relaxed)
}
fn run() -> Result<()> {
    let opt = Options::parse_from(std::env::args_os().map(|arg| {
        let normalized = arg.to_string_lossy().to_ascii_lowercase();
        let mapped = match normalized.as_str() {
            "-workspacepath" => "--workspace",
            "-model" | "-modelpath" => "--model",
            "-contextlength" => "--context-length",
            "-cputhreads" => "--cpu-threads",
            "-batchthreads" => "--batch-threads",
            "-gpureservemib" => "--gpu-reserve-mib",
            "-kvcachetype" => "--kv-cache-type",
            "-speculative" => "--speculative",
            "-draftmodelpath" => "--draft-model",
            "-reasoningeffort" => "--reasoning-effort",
            "-thinkingbudget" => "--thinking-budget",
            "-thinkingdisplay" => "--thinking-display",
            "-enableweb" => "--enable-web",
            "-disableweb" => "--disable-web",
            _ => return arg,
        };
        mapped.into()
    }));
    let max = std::thread::available_parallelism().map_or(1, usize::from);
    for n in [opt.cpu_threads, opt.batch_threads].into_iter().flatten() {
        ensure!(
            n > 0 && n <= max,
            "Thread count must be between 1 and {max}"
        );
    }
    if let Some(p) = &opt.draft_model {
        ensure!(
            p.is_file() && matches!(opt.speculative.as_str(), "auto" | "draft-mtp"),
            "Draft model requires an existing GGUF and compatible speculative mode"
        );
    }
    let home = if let Some(p) = &opt.home {
        dunce::canonicalize(p)?
    } else {
        std::env::current_exe()?
            .ancestors()
            .skip(1)
            .find(|p| {
                p.join("runtime/llama.cpp").is_dir() || p.join("scripts/start-agent.ps1").is_file()
            })
            .context("Cannot locate installation. Set LUMA_HOME or pass --home <folder>")?
            .to_owned()
    };
    ctrlc::set_handler(|| INTERRUPTED.store(true, Ordering::Relaxed))?;
    if let Some(action) = &opt.action {
        return action.run(&home, &opt);
    }
    agent::Agent::open(opt, home)?.run()
}
fn main() {
    if let Err(e) = run() {
        eprintln!("Luma: {}", safe(&format!("{e:#}")));
        std::process::exit(1);
    }
}
