use crate::{clip, safe};
use regex::Regex;
use serde_json::{Value, json};
use std::{collections::HashSet, sync::LazyLock};
use uuid::Uuid;
pub const SUMMARY: &str = "[Conversation summary]";
pub fn content(m: &Value) -> &str {
    m["content"].as_str().unwrap_or("")
}
fn call(name: &str, args: &Value) -> Value {
    json!({"id":Uuid::new_v4().simple().to_string(),"type":"function","function":{"name":name,"arguments":args.to_string()}})
}
pub fn args(c: &Value) -> Option<Value> {
    let raw = &c["function"]["arguments"];
    let v = if let Some(s) = raw.as_str() {
        serde_json::from_str(s).ok()?
    } else {
        raw.clone()
    };
    v.is_object().then_some(v)
}
// Resolve each call exactly once, including interrupted calls in old transcripts.
pub fn clean(raw: Vec<Value>) -> Vec<Value> {
    let mut out = Vec::new();
    let mut expected: Vec<String> = Vec::new();
    fn close(out: &mut Vec<Value>, expected: &mut Vec<String>) {
        for id in expected.drain(..) {
            out.push(json!({"role":"tool","tool_call_id":id,"content":"Interrupted before a tool result was recorded."}));
        }
    }
    for m in raw {
        let role = m["role"].as_str().unwrap_or("");
        if role == "tool" {
            if let Some(i) = expected
                .iter()
                .position(|id| Some(id.as_str()) == m["tool_call_id"].as_str())
            {
                expected.remove(i);
                out.push(
                    json!({"role":"tool","tool_call_id":m["tool_call_id"],"content":content(&m)}),
                );
            }
            continue;
        }
        close(&mut out, &mut expected);
        match role {
            "system" if !out.is_empty() => {
                out.push(json!({"role":"user","content":format!("{SUMMARY} {}",content(&m))}));
                out.push(json!({"role":"assistant","content":"Understood."}));
            }
            "user" => {
                if let Some(prev) = out.last_mut().filter(|p| p["role"] == "user") {
                    prev["content"] = format!("{}\n\n{}", content(prev), content(&m)).into();
                } else {
                    out.push(json!({"role":"user","content":content(&m)}));
                }
            }
            "assistant" => {
                let mut msg = json!({"role":"assistant","content":content(&m)});
                if let Some(s) = m["reasoning_content"].as_str() {
                    msg["reasoning_content"] = s.into();
                }
                if let Some(cs) = m["tool_calls"].as_array() {
                    let mut list = Vec::new();
                    for c in cs {
                        if let Some(n) = c["function"]["name"].as_str() {
                            let mut fixed = call(n, &args(c).unwrap_or(json!({})));
                            if let Some(id) = c["id"]
                                .as_str()
                                .filter(|id| !id.is_empty() && !expected.iter().any(|s| s == id))
                            {
                                fixed["id"] = id.into();
                            }
                            expected.push(fixed["id"].as_str().unwrap().into());
                            list.push(fixed);
                        }
                    }
                    if !list.is_empty() {
                        msg["tool_calls"] = list.into();
                    }
                }
                if !content(&msg).is_empty()
                    || msg["tool_calls"].is_array()
                    || !msg["reasoning_content"].is_null()
                {
                    out.push(msg);
                }
            }
            _ => {}
        }
    }
    close(&mut out, &mut expected);
    out
}

static XML: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<tool_call>\s*(.*?)\s*</tool_call>").unwrap());
static FUNCTION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<function\s*=\s*([^>\s]+)\s*>(.*?)</function>").unwrap());
static PARAM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<parameter\s*=\s*([^>\s]+)\s*>(.*?)</parameter>").unwrap());
static FENCE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)```([^\n`]*)\n(.*?)```").unwrap());
static INLINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b([a-z_]+)\s*\(\s*(\{)").unwrap());
fn payload(v: &Value, allowed: &HashSet<&str>, out: &mut Vec<Value>) {
    if let Some(arr) = v.as_array() {
        for i in arr {
            payload(i, allowed, out)
        }
        return;
    }
    let v = v.get("function").unwrap_or(v);
    if let Some(n) = v["name"]
        .as_str()
        .or(v["tool"].as_str())
        .filter(|n| allowed.contains(*n))
    {
        let raw = v
            .get("arguments")
            .or(v.get("parameters"))
            .cloned()
            .unwrap_or(json!({}));
        let a = if let Some(s) = raw.as_str() {
            serde_json::from_str(s).unwrap_or(Value::Null)
        } else {
            raw
        };
        if a.is_object() {
            out.push(call(n, &a));
        }
    }
}
pub fn legacy(
    m: &mut Value,
    allowed: &HashSet<&str>,
    user: &str,
    ws: &crate::workspace::Workspace,
) {
    if m["tool_calls"].is_array() {
        return;
    }
    let s = content(m);
    let mut calls = Vec::new();
    let mut spans = Vec::new();
    for c in XML.captures_iter(s) {
        let before = calls.len();
        if let Ok(v) = serde_json::from_str(&c[1]) {
            payload(&v, allowed, &mut calls);
        } else {
            for f in FUNCTION.captures_iter(&c[1]) {
                if allowed.contains(&f[1]) {
                    let mut a = json!({});
                    for p in PARAM.captures_iter(&f[2]) {
                        let t = p[2].trim();
                        a[&p[1]] = if matches!(
                            &p[1],
                            "max_entries"
                                | "max_depth"
                                | "max_results"
                                | "start_line"
                                | "end_line"
                                | "timeout_seconds"
                        ) {
                            serde_json::from_str(t).unwrap_or_else(|_| json!(t))
                        } else {
                            json!(t)
                        };
                    }
                    calls.push(call(&f[1], &a));
                }
            }
        }
        if calls.len() > before {
            spans.push(c.get(0).unwrap().range());
        }
    }
    for c in FENCE.captures_iter(s) {
        let span = c.get(0).unwrap().range();
        if spans.iter().any(|r| r.contains(&span.start)) {
            continue;
        }
        if matches!(c[1].trim(), "" | "json" | "tool_call")
            && let Ok(v) = serde_json::from_str(c[2].trim())
        {
            let before = calls.len();
            payload(&v, allowed, &mut calls);
            if before < calls.len() {
                spans.push(span);
            }
        }
    }
    for c in INLINE.captures_iter(s) {
        let start = c.get(0).unwrap().start();
        if !allowed.contains(&c[1]) || spans.iter().any(|r| r.contains(&start)) {
            continue;
        }
        let args_start = c.get(2).unwrap().start();
        let mut reader = serde_json::Deserializer::from_str(&s[args_start..]).into_iter::<Value>();
        if let Some(Ok(v)) = reader.next() {
            let end = args_start + reader.byte_offset();
            let tail = &s[end..];
            if tail.trim_start().starts_with(')') {
                calls.push(call(&c[1], &v));
                spans.push(start..end + (tail.len() - tail.trim_start().len()) + 1);
            }
        }
    }
    if calls.is_empty()
        && let Ok(v) = serde_json::from_str(s.trim())
    {
        payload(&v, allowed, &mut calls);
        if !calls.is_empty() {
            spans.push(0..s.len());
        }
    }
    // Infer a new file only for an explicit creation request naming one compatible file.
    if calls.is_empty() && allowed.contains("write_file") {
        static CREATE: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"(?i)\b(create|write|save|generate|build)\b").unwrap());
        static FILE: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r"(?i)([\w./\\-]+\.(?:py|rs|js|ts|html|css|json|ps1|md|txt))\b").unwrap()
        });
        let paths: HashSet<_> = FILE.find_iter(user).map(|m| m.as_str()).collect();
        if CREATE.is_match(user)
            && paths.len() == 1
            && let Some(&p) = paths.iter().next()
            && ws.path(p).is_ok_and(|p| !p.exists())
        {
            let mut fences = FENCE.captures_iter(s);
            if let Some(c) = fences.next()
                && fences.next().is_none()
            {
                let lang = c[1].trim().to_lowercase();
                let ext = p.rsplit('.').next().unwrap_or("").to_lowercase();
                let canonical = match lang.as_str() {
                    "python" => "py",
                    "rust" => "rs",
                    "javascript" => "js",
                    "typescript" => "ts",
                    "powershell" => "ps1",
                    "markdown" => "md",
                    "text" => "txt",
                    l => l,
                };
                if canonical == ext {
                    calls.push(call(
                        "write_file",
                        &json!({"path":p,"content":format!("{}\n",c[2].trim_end())}),
                    ));
                    spans.push(c.get(0).unwrap().range());
                }
            }
        }
    }
    if !calls.is_empty() {
        let mut cleaned = s.to_string();
        spans.sort_by_key(|r| r.start);
        let mut bound = s.len();
        for r in spans.into_iter().rev() {
            if r.end <= bound {
                bound = r.start;
                cleaned.replace_range(r, "");
            }
        }
        m["content"] = cleaned.trim().into();
        m["tool_calls"] = calls.into();
    }
}
pub fn estimate(msgs: &[Value]) -> usize {
    msgs.iter().map(|m| m.to_string().len() / 3 + 8).sum()
}
pub fn compact(msgs: &mut Vec<Value>, target: usize) -> bool {
    let mut changed = false;
    let Some(start) = msgs
        .iter()
        .rposition(|m| m["role"] == "user" && !content(m).starts_with(SUMMARY))
    else {
        return false;
    };
    if start > 0 && !(start == 2 && content(&msgs[0]).starts_with(SUMMARY)) {
        let mut notes = Vec::new();
        for m in &msgs[..start] {
            let role = m["role"].as_str().unwrap_or("");
            if role == "user" || role == "assistant" && !content(m).is_empty() {
                notes.push(format!(
                    "{role}: {}",
                    clip(
                        content(m),
                        if content(m).starts_with(SUMMARY) {
                            650
                        } else {
                            240
                        }
                    )
                ));
            }
            if let Some(cs) = m["tool_calls"].as_array() {
                for c in cs {
                    let a = args(c).unwrap_or_default();
                    let which = a["path"].as_str().or(a["command"].as_str()).unwrap_or("");
                    notes.push(format!(
                        "Requested {} {}",
                        c["function"]["name"],
                        clip(which, 120)
                    ));
                }
            }
            if role == "tool" {
                notes.push(format!("Tool result: {}", clip(content(m), 160)));
            }
        }
        let summary = format!(
            "{SUMMARY}\n{}",
            notes[notes.len().saturating_sub(20)..].join("\n")
        );
        msgs.splice(..start,[json!({"role":"user","content":summary}),json!({"role":"assistant","content":"Understood. I will continue from this summary."})]);
        changed = true;
    }
    if estimate(msgs) > target {
        for m in msgs.iter_mut() {
            if let Some(obj) = m.as_object_mut() {
                changed |= obj.remove("reasoning_content").is_some();
            }
        }
        for i in 0..msgs.len() {
            if estimate(msgs) <= target {
                break;
            }
            if msgs[i]["role"] == "tool" && content(&msgs[i]).chars().count() > 1600 {
                msgs[i]["content"] = format!(
                    "{}\n[older tool output truncated]",
                    clip(content(&msgs[i]), 1500)
                )
                .into();
                changed = true;
            }
        }
    }
    changed
}

// Keep a partial marker between chunks. Ordinary code streams immediately; a
// JSON/XML-looking fence is held until we can distinguish an answer from a call.
pub struct Display<'a> {
    buffer: String,
    end: &'static str,
    show: bool,
    allowed: HashSet<&'a str>,
    fence_header: Option<String>,
}
impl<'a> Display<'a> {
    pub fn new(show: bool, tools: &'a Value) -> Self {
        let allowed = tools
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|t| t["function"]["name"].as_str())
            .collect();
        Self {
            buffer: String::new(),
            end: "",
            show,
            allowed,
            fence_header: None,
        }
    }
    pub fn feed(&mut self, s: &str, final_chunk: bool) {
        self.buffer.push_str(s);
        loop {
            if self.end == "header" {
                let Some(i) = self.buffer.find('\n') else {
                    if final_chunk {
                        print!("```{}", safe(&self.buffer));
                        self.buffer.clear();
                    }
                    break;
                };
                let header = self.buffer.drain(..=i).collect::<String>();
                self.end = "```";
                if matches!(header.trim(), "" | "json" | "xml" | "tool_call") {
                    self.fence_header = Some(header);
                } else {
                    print!("```{}", safe(&header));
                }
            }
            if let Some(header) = &self.fence_header {
                let inner = self.buffer.trim_start();
                if !inner.is_empty() && !inner.starts_with(['{', '[', '<']) {
                    print!("```{}", safe(header));
                    self.fence_header = None;
                } else if let Some(i) = self
                    .buffer
                    .find("```")
                    .or_else(|| final_chunk.then_some(self.buffer.len()))
                {
                    let body = &self.buffer[..i];
                    let mut found = Vec::new();
                    if let Ok(v) = serde_json::from_str(body.trim()) {
                        payload(&v, &self.allowed, &mut found);
                    }
                    let close = if i < self.buffer.len() { "```" } else { "" };
                    if found.is_empty() && !body.trim_start().starts_with("<tool_call>") {
                        print!("```{}{}{}", safe(header), safe(body), close);
                    }
                    self.buffer.drain(..i + close.len());
                    self.fence_header = None;
                    self.end = "";
                    continue;
                } else {
                    break;
                }
            }
            let single = [self.end];
            let markers: &[&str] = if self.end.is_empty() {
                &["<think>", "<tool_call>", "```"]
            } else {
                &single
            };
            let found = markers
                .iter()
                .filter_map(|&m| self.buffer.find(m).map(|pos| (pos, m)))
                .min_by_key(|p| p.0);
            let (end, marker) = if let Some((pos, m)) = found {
                (pos, Some(m))
            } else {
                let len = self.buffer.len();
                let held = if final_chunk {
                    0
                } else {
                    (1..markers.iter().map(|m| m.len()).max().unwrap_or(1))
                        .rev()
                        .find(|&n| {
                            n <= len
                                && markers
                                    .iter()
                                    .any(|m| n < m.len() && self.buffer.ends_with(&m[..n]))
                        })
                        .unwrap_or(0)
                };
                (len - held, None)
            };
            let piece = safe(&self.buffer[..end]);
            if self.end.is_empty() || self.end == "```" {
                print!("{piece}");
            } else if self.end == "</think>" && self.show {
                print!("{}", crate::ui::muted(&piece));
            }
            self.buffer.drain(..end + marker.map_or(0, str::len));
            let Some(m) = marker else { break };
            self.end = match m {
                "```" if self.end == "```" => {
                    print!("```");
                    ""
                }
                "```" => "header",
                "<think>" => "</think>",
                "<tool_call>" => "</tool_call>",
                _ => "",
            };
        }
    }
}
