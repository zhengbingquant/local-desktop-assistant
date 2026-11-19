use crate::{Options, model::Model, process::Process, safe};
use anyhow::{Context, Result, bail, ensure};
use reqwest::{blocking::Client, redirect::Policy};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
    net::TcpListener,
    path::Path,
    process::{Command, Stdio},
    sync::mpsc::{RecvTimeoutError, sync_channel},
    time::{Duration, Instant},
};
use tempfile::TempDir;
use uuid::Uuid;

#[derive(Debug)]
pub struct Overflow;
impl std::fmt::Display for Overflow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "The prompt exceeds the model context")
    }
}
impl std::error::Error for Overflow {}
pub struct Reply {
    pub message: Value,
    pub finish: String,
    pub usage: Option<usize>,
    pub speed: Option<f64>,
}
pub struct Server {
    process: Process,
    logs: TempDir,
    client: Client,
    url: String,
    key: String,
}
impl Server {
    pub fn start(home: &Path, opt: &Options, m: &Model) -> Result<Self> {
        let exe = home.join("runtime/llama.cpp/llama-server.exe");
        ensure!(
            exe.is_file(),
            "Missing llama-server. Run scripts/install-runtime.ps1."
        );
        let port = TcpListener::bind(("127.0.0.1", 0))?.local_addr()?.port();
        let key = Uuid::new_v4().simple().to_string();
        let logs = tempfile::tempdir()?;
        let log = File::create(logs.path().join("server.log"))?;
        let mut cmd = Command::new(&exe);
        cmd.current_dir(exe.parent().unwrap())
            .arg("--model")
            .arg(&m.path)
            .args([
                "--host",
                "127.0.0.1",
                "--port",
                &port.to_string(),
                "--api-key",
                &key,
                "--ctx-size",
                &m.ctx.to_string(),
                "--parallel",
                "1",
                "--fit",
                "on",
                "--fit-target",
                &opt.gpu_reserve_mib.to_string(),
                "--flash-attn",
                "on",
                "--batch-size",
                if m.resident { "256" } else { "512" },
                "--ubatch-size",
                if m.resident { "128" } else { "512" },
                "--cache-type-k",
                &opt.kv_cache_type,
                "--cache-type-v",
                &opt.kv_cache_type,
                "--jinja",
                "--offline",
                "--no-webui",
                "--no-agent",
                "--reasoning",
                if m.reasoning { "on" } else { "off" },
            ]);
        if let Some(n) = m.threads {
            cmd.args(["--threads", &n.to_string()]);
        }
        if let Some(n) = opt.batch_threads {
            cmd.args(["--threads-batch", &n.to_string()]);
        }
        if m.spec != "none" {
            cmd.args(["--spec-type", &m.spec]);
        }
        if m.spec == "draft-mtp" {
            cmd.args(["--spec-draft-n-max", if m.resident { "3" } else { "1" }]);
            if let Some(p) = &opt.draft_model {
                cmd.arg("--model-draft").arg(p);
            }
        }
        cmd.stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log);
        let client = Client::builder()
            .no_proxy()
            .redirect(Policy::none())
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(3600))
            .build()?;
        let mut server = Self {
            process: Process::start(&mut cmd)?,
            logs,
            client,
            url: format!("http://127.0.0.1:{port}"),
            key,
        };
        println!("Loading {} locally…", safe(&m.name));
        let start = Instant::now();
        loop {
            ensure!(!crate::interrupted(), "Loading interrupted");
            if server.process.child.try_wait()?.is_some() {
                bail!("Model server stopped:\n{}", safe(&server.tail()));
            }
            if let Ok(resp) = server
                .client
                .get(format!("{}/health", server.url))
                .bearer_auth(&server.key)
                .timeout(Duration::from_secs(2))
                .send()
            {
                if resp.status().is_success() {
                    break;
                }
                ensure!(
                    resp.status().as_u16() == 503,
                    "Unexpected server health status {}",
                    resp.status()
                );
            }
            ensure!(
                start.elapsed() < Duration::from_secs(900),
                "Model server startup timed out"
            );
            std::thread::sleep(Duration::from_millis(200));
        }
        Ok(server)
    }
    fn tail(&self) -> String {
        let mut s = Vec::new();
        if let Ok(mut f) = File::open(self.logs.path().join("server.log")) {
            if let Ok(meta) = f.metadata() {
                let _ = f.seek(SeekFrom::Start(meta.len().saturating_sub(12_000)));
            }
            let _ = f.read_to_end(&mut s);
        }
        String::from_utf8_lossy(&s).into_owned()
    }
    pub fn clear_log(&self) -> Result<()> {
        File::options()
            .write(true)
            .open(self.logs.path().join("server.log"))?
            .set_len(0)?;
        Ok(())
    }
    pub fn generate(&self, body: Value, show: bool) -> Result<Reply> {
        let req = self
            .client
            .post(format!("{}/v1/chat/completions", self.url))
            .bearer_auth(&self.key)
            .json(&body);
        let (tx, rx) = sync_channel::<Result<Value>>(16);
        std::thread::spawn(move || {
            let run = || -> Result<()> {
                let response = req.send().context("Local inference request failed")?;
                if !response.status().is_success() {
                    let code = response.status();
                    let mut detail = String::new();
                    response.take(16_000).read_to_string(&mut detail)?;
                    if code.as_u16() == 400
                        && (detail.contains("exceed_context_size")
                            || detail.contains("exceeds the available context")
                            || detail.contains("context size"))
                    {
                        return Err(Overflow.into());
                    }
                    bail!("Local server {code}: {}", safe(&detail))
                }
                for line in BufReader::new(response).lines() {
                    let line = line?;
                    let Some(data) = line.strip_prefix("data:").map(str::trim) else {
                        continue;
                    };
                    if data == "[DONE]" {
                        break;
                    }
                    if !data.is_empty() && tx.send(Ok(serde_json::from_str(data)?)).is_err() {
                        break;
                    }
                }
                Ok(())
            };
            if let Err(e) = run() {
                let _ = tx.send(Err(e));
            }
        });
        let mut content = String::new();
        let mut reasoning = String::new();
        let mut calls: BTreeMap<u64, (String, String, String)> = BTreeMap::new();
        let mut finish = None;
        let mut usage = None;
        let mut speed = None;
        let mut display = crate::history::Display::new(show, &body["tools"]);
        loop {
            ensure!(!crate::interrupted(), "Generation interrupted");
            let v = match rx.recv_timeout(Duration::from_millis(60)) {
                Ok(v) => v?,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => break,
            };
            if !v["error"].is_null() {
                bail!("Local model: {}", safe(&v["error"].to_string()));
            }
            if let (Some(p), Some(c)) = (
                v["usage"]["prompt_tokens"].as_u64(),
                v["usage"]["completion_tokens"].as_u64(),
            ) {
                usage = Some((p + c) as usize);
            }
            if let Some(n) = v["timings"]["predicted_per_second"].as_f64() {
                speed = Some(n);
            }
            let ch = &v["choices"][0];
            if let Some(s) = ch["finish_reason"].as_str() {
                finish = Some(s.to_owned());
            }
            let d = &ch["delta"];
            if let Some(s) = d["reasoning_content"].as_str().or(d["reasoning"].as_str()) {
                reasoning.push_str(s);
                if show {
                    print!("{}", crate::ui::muted(&safe(s)));
                }
            }
            if let Some(s) = d["content"].as_str() {
                content.push_str(s);
                display.feed(s, false);
            }
            if let Some(parts) = d["tool_calls"].as_array() {
                for frag in parts {
                    let call = calls
                        .entry(frag["index"].as_u64().unwrap_or(0))
                        .or_default();
                    if let Some(id) = frag["id"].as_str() {
                        call.0 = id.to_owned();
                    }
                    if let Some(n) = frag["function"]["name"].as_str() {
                        call.1.push_str(n);
                    }
                    let args = &frag["function"]["arguments"];
                    if let Some(s) = args.as_str() {
                        call.2.push_str(s);
                    } else if !args.is_null() {
                        call.2.push_str(&args.to_string());
                    }
                }
            }
            std::io::stdout().flush()?;
        }
        display.feed("", true);
        println!();
        let finish = finish.context("Incomplete generation. Partial tool calls were discarded.")?;
        let mut message = json!({"role":"assistant","content":content});
        if !reasoning.is_empty() {
            message["reasoning_content"] = reasoning.into();
        }
        if !calls.is_empty() {
            message["tool_calls"]=calls.into_values().map(|(id,name,args)|json!({"id":if id.is_empty(){Uuid::new_v4().simple().to_string()}else{id},"type":"function","function":{"name":name,"arguments":args}})).collect::<Vec<_>>().into();
        }
        Ok(Reply {
            message,
            finish,
            usage,
            speed,
        })
    }
}
