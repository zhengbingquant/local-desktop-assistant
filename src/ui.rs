use crate::{Mode, safe};
use anyhow::Result;
use crossterm::{
    cursor::{Hide, MoveToColumn, MoveUp, Show},
    event::{
        self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEventKind,
        KeyModifiers,
    },
    execute, queue,
    terminal::{self, Clear, ClearType},
};
use std::io::{self, IsTerminal, Write};
use unicode_width::UnicodeWidthChar;

pub const COMMANDS: &[(&str, &str)] = &[
    ("/mode", "Choose how the agent can act"),
    ("/plan", "Inspect files and build a plan"),
    ("/permissions", "Review approval settings"),
    ("/model", "Select a local GGUF model"),
    ("/effort", "Set reasoning effort or token budget"),
    ("/think", "Show or hide reasoning"),
    ("/resume", "Reopen a saved conversation"),
    ("/new", "Start a fresh conversation"),
    ("/history", "Read the saved transcript"),
    ("/compact", "Summarize older turns"),
    ("/context", "Inspect token usage"),
    ("/workspace", "Show or change project folder"),
    ("/web", "Toggle public web research"),
    ("/save", "Save this conversation"),
    ("/delete", "Delete saved conversations"),
    ("/clear", "Start a fresh conversation"),
    ("/cls", "Clear the terminal screen"),
    ("/help", "Browse commands and shortcuts"),
    ("/exit", "Close Luma"),
];
pub fn interactive() -> bool {
    io::stdin().is_terminal() && io::stdout().is_terminal()
}
fn paint(s: &str, color: &str) -> String {
    if interactive() && std::env::var_os("NO_COLOR").is_none() {
        format!("\x1b[{color}m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}
pub fn accent(s: &str) -> String {
    paint(s, "1;38;5;216")
}
pub fn muted(s: &str) -> String {
    paint(s, "38;5;245")
}
pub fn welcome(workspace: &std::path::Path, model: &str) {
    println!(
        "\n  {}  {}",
        accent("Luma"),
        muted(env!("CARGO_PKG_VERSION"))
    );
    println!(
        "  {}\n  {}\n",
        safe(&workspace.display().to_string()),
        muted(&format!("{} · local inference", safe(model)))
    );
}
pub fn help() {
    for (cmd, desc) in COMMANDS {
        println!("  {:14} {}", cmd, desc);
    }
    println!(
        "\n  Shift+Tab  cycle modes     Tab  complete     Esc  dismiss\n  Alt+Enter  new line        ↑ ↓  navigate     Ctrl+C  interrupt"
    );
}
pub fn confirm(prompt: &str) -> Result<bool> {
    print!("{} {} [y/N] ", accent("?"), safe(prompt));
    io::stdout().flush()?;
    let mut s = String::new();
    io::stdin().read_line(&mut s)?;
    Ok(matches!(s.trim().to_lowercase().as_str(), "y" | "yes"))
}
struct Raw;
impl Raw {
    fn start() -> Result<Self> {
        terminal::enable_raw_mode()?;
        let guard = Self;
        if let Err(e) = execute!(io::stdout(), EnableBracketedPaste)
            && e.kind() != io::ErrorKind::Unsupported
        {
            return Err(e.into());
        }
        Ok(guard)
    }
}
impl Drop for Raw {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), DisableBracketedPaste, Show);
        let _ = terminal::disable_raw_mode();
    }
}
fn width() -> usize {
    terminal::size()
        .map_or(80, |(w, _)| w as usize)
        .saturating_sub(1)
        .clamp(10, 160)
}
fn fit(s: &str, w: usize) -> String {
    let mut used = 0;
    s.chars()
        .take_while(|c| {
            used += c.width().unwrap_or(0);
            used <= w
        })
        .collect()
}
fn window(selected: usize, count: usize) -> std::ops::Range<usize> {
    let rows = terminal::size().map_or(6, |(_, h)| (h as usize).saturating_sub(6).clamp(1, 7));
    let start = selected.saturating_sub(rows - 1);
    start..(start + rows).min(count)
}
pub fn choose(title: &str, options: &[(String, String)], current: usize) -> Result<Option<usize>> {
    if options.is_empty() {
        return Ok(None);
    }
    if !interactive() {
        println!("{title}");
        for (i, (name, desc)) in options.iter().enumerate() {
            println!("  {}. {}  {}", i + 1, safe(name), safe(desc));
        }
        return Ok(None);
    }
    let _raw = Raw::start()?;
    let mut stdout = io::stdout();
    let mut idx = current.min(options.len() - 1);
    let mut rows = 0;
    let mut filter = String::new();
    loop {
        let matches: Vec<_> = options
            .iter()
            .enumerate()
            .filter(|(_, (n, d))| {
                format!("{n} {d}")
                    .to_lowercase()
                    .contains(&filter.to_lowercase())
            })
            .collect();
        idx = idx.min(matches.len().saturating_sub(1));
        let w = width();
        queue!(
            stdout,
            Hide,
            MoveToColumn(0),
            Clear(ClearType::FromCursorDown)
        )?;
        write!(
            stdout,
            "{}",
            accent(&fit(&format!("{title}  {}", safe(&filter)), w))
        )?;
        for i in window(idx, matches.len()) {
            let (_, (name, desc)) = matches[i];
            let line = fit(
                &format!(
                    " {} {}   {}",
                    if i == idx { "❯" } else { " " },
                    safe(name),
                    safe(desc)
                ),
                w,
            );
            write!(
                stdout,
                "\r\n{}",
                if i == idx {
                    accent(&line)
                } else {
                    muted(&line)
                }
            )?;
            rows += 1;
        }
        write!(
            stdout,
            "\r\n{}",
            muted(&fit(
                "  type to filter · ↑↓ select · enter confirm · esc cancel",
                w
            ))
        )?;
        rows += 1;
        queue!(stdout, MoveUp(rows), MoveToColumn(0))?;
        stdout.flush()?;
        rows = 0;
        let Event::Key(k) = event::read()? else {
            continue;
        };
        if k.kind == KeyEventKind::Release {
            continue;
        }
        let done = match k.code {
            KeyCode::Esc => Some(None),
            KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => Some(None),
            KeyCode::Enter => Some(matches.get(idx).map(|x| x.0)),
            KeyCode::Up => {
                idx = if idx == 0 {
                    matches.len().saturating_sub(1)
                } else {
                    idx - 1
                };
                None
            }
            KeyCode::Down => {
                idx = (idx + 1) % matches.len().max(1);
                None
            }
            KeyCode::Backspace => {
                filter.pop();
                idx = 0;
                None
            }
            KeyCode::Char(c) if !c.is_control() => {
                filter.push(c);
                idx = 0;
                None
            }
            _ => None,
        };
        if let Some(result) = done {
            execute!(stdout, Clear(ClearType::FromCursorDown), Show)?;
            return Ok(result);
        }
    }
}

pub fn prompt(
    mode: &mut Mode,
    model: &str,
    usage: &str,
    web: bool,
    history: &[String],
) -> Result<Option<String>> {
    if !interactive() {
        let mut s = String::new();
        return Ok((io::stdin().read_line(&mut s)? > 0).then(|| s.trim_end().to_owned()));
    }
    let _raw = Raw::start()?;
    let mut out = io::stdout();
    write!(out, "\r\n{}\r\n", muted(&"─".repeat(width())))?;
    let mut pending = std::collections::VecDeque::new();
    let mut buf: Vec<char> = Vec::new();
    let mut cursor: usize = 0;
    let mut selected = 0;
    let mut dismissed = false;
    let mut hi = history.len();
    let mut draft = String::new();
    loop {
        let input: String = buf.iter().collect();
        let w = width();
        let choices: Vec<_> = COMMANDS
            .iter()
            .filter(|(c, _)| {
                !dismissed
                    && input.starts_with('/')
                    && !input.contains(char::is_whitespace)
                    && c.starts_with(&input.to_lowercase())
            })
            .collect();
        selected = selected.min(choices.len().saturating_sub(1));
        let status = fit(
            &format!(
                " {}  ·  {}  ·  {}  ·  web {}",
                mode.label(),
                safe(model),
                usage,
                if web { "on" } else { "off" }
            ),
            w,
        );
        let mut start = cursor;
        let mut pos = 0;
        while start > 0 {
            let a = buf[start - 1].width().unwrap_or(1).max(1);
            if pos + a >= w.saturating_sub(3) {
                break;
            }
            pos += a;
            start -= 1;
        }
        let visible: String = buf[start..]
            .iter()
            .map(|&c| {
                if c == '\n' {
                    '↵'
                } else if c.is_control() {
                    ' '
                } else {
                    c
                }
            })
            .collect();
        let displayed = fit(&visible, w - 2);
        queue!(out, Hide, MoveToColumn(0), Clear(ClearType::FromCursorDown))?;
        write!(
            out,
            "{} {}",
            accent("❯"),
            if input.is_empty() {
                muted(&fit("Ask about this project, or type /", w - 2))
            } else {
                displayed
            }
        )?;
        let mut rows = 0;
        if !choices.is_empty() {
            for i in window(selected, choices.len()) {
                let (cmd, desc) = choices[i];
                let line = fit(
                    &format!(" {} {cmd:13} {desc}", if selected == i { "›" } else { " " }),
                    w,
                );
                write!(
                    out,
                    "\r\n{}",
                    if i == selected {
                        accent(&line)
                    } else {
                        muted(&line)
                    }
                )?;
                rows += 1;
            }
        }
        write!(out, "\r\n{}\r\n{}", muted(&"─".repeat(w)), muted(&status))?;
        rows += 2;
        let col: usize = buf[start..cursor]
            .iter()
            .map(|c| {
                if c.is_control() {
                    1
                } else {
                    c.width().unwrap_or(0)
                }
            })
            .sum();
        queue!(
            out,
            MoveUp(rows),
            MoveToColumn((2 + col).min(w) as u16),
            Show
        )?;
        out.flush()?;
        let mut submit = None;
        match input_event(&mut pending)? {
            Event::Paste(s) => {
                let chars: Vec<_> = s
                    .replace("\r\n", "\n")
                    .replace('\r', "\n")
                    .chars()
                    .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
                    .collect();
                let n = chars.len();
                buf.splice(cursor..cursor, chars);
                cursor += n;
                dismissed = true;
            }
            Event::Key(k) if k.kind != KeyEventKind::Release => match k.code {
                KeyCode::BackTab => {
                    *mode = mode.next();
                }
                KeyCode::Tab if k.modifiers.contains(KeyModifiers::SHIFT) => {
                    *mode = mode.next();
                }
                KeyCode::Tab if !choices.is_empty() => {
                    buf = choices[selected].0.chars().collect();
                    cursor = buf.len();
                }
                KeyCode::Esc => {
                    dismissed = true;
                }
                KeyCode::Enter
                    if k.modifiers
                        .intersects(KeyModifiers::ALT | KeyModifiers::SHIFT) =>
                {
                    buf.insert(cursor, '\n');
                    cursor += 1;
                }
                KeyCode::Enter => {
                    let text = if !choices.is_empty() {
                        choices[selected].0.to_string()
                    } else {
                        input
                    };
                    submit = Some(Some(text));
                }
                KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                    if buf.is_empty() {
                        submit = Some(None)
                    } else {
                        buf.clear();
                        cursor = 0;
                        dismissed = false;
                    }
                }
                KeyCode::Char('d')
                    if k.modifiers.contains(KeyModifiers::CONTROL) && buf.is_empty() =>
                {
                    submit = Some(None)
                }
                KeyCode::Up | KeyCode::Down if !choices.is_empty() => {
                    if k.code == KeyCode::Up {
                        selected = (selected + choices.len() - 1) % choices.len();
                    } else {
                        selected = (selected + 1) % choices.len();
                    }
                }
                KeyCode::Up | KeyCode::Down => {
                    if hi == history.len() {
                        draft = input;
                    }
                    hi = if k.code == KeyCode::Up {
                        hi.saturating_sub(1)
                    } else {
                        (hi + 1).min(history.len())
                    };
                    buf = history.get(hi).unwrap_or(&draft).chars().collect();
                    cursor = buf.len();
                    dismissed = true;
                }
                KeyCode::Left => cursor = cursor.saturating_sub(1),
                KeyCode::Right => cursor = (cursor + 1).min(buf.len()),
                KeyCode::Home => cursor = 0,
                KeyCode::End => cursor = buf.len(),
                KeyCode::Backspace if cursor > 0 => {
                    cursor -= 1;
                    buf.remove(cursor);
                    dismissed = false;
                    selected = 0;
                }
                KeyCode::Delete if cursor < buf.len() => {
                    buf.remove(cursor);
                }
                KeyCode::Char(c)
                    if !c.is_control() && !k.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    buf.insert(cursor, c);
                    cursor += 1;
                    selected = 0;
                    dismissed = false;
                }
                _ => {}
            },
            _ => {}
        }
        if let Some(result) = submit {
            queue!(out, MoveToColumn(0), Clear(ClearType::FromCursorDown))?;
            if let Some(s) = &result {
                write!(out, "{} {}\r\n", accent("❯"), safe(s).replace('\n', "\r\n"))?;
            }
            out.flush()?;
            return Ok(result);
        }
    }
}

fn input_event(pending: &mut std::collections::VecDeque<Event>) -> Result<Event> {
    if let Some(e) = pending.pop_front() {
        return Ok(e);
    }
    let first = event::read()?;
    #[cfg(windows)]
    if let Event::Key(k) = first
        && k.kind != KeyEventKind::Release
    {
        if k.code == KeyCode::Char('v')
            && k.modifiers
                .intersects(KeyModifiers::ALT | KeyModifiers::CONTROL)
        {
            return Ok(Event::Paste(
                clipboard_win::get_clipboard_string().unwrap_or_default(),
            ));
        }
        // Windows Terminal can deliver the VT paste markers as separate console
        // key events. Collect the entire paste before Enter is interpreted.
        if k.code == KeyCode::Esc || k.code == KeyCode::Char('\x1b') {
            for expected in "[200~".chars() {
                let next = loop {
                    if !event::poll(std::time::Duration::from_millis(20))? {
                        return Ok(first);
                    }
                    let next = event::read()?;
                    if !matches!(next, Event::Key(key) if key.kind == KeyEventKind::Release) {
                        break next;
                    }
                };
                let matched =
                    matches!(next, Event::Key(key) if key.code == KeyCode::Char(expected));
                pending.push_back(next);
                if !matched {
                    return Ok(first);
                }
            }
            pending.clear();
            let mut text = String::new();
            while event::poll(std::time::Duration::from_secs(2))? {
                if let Event::Key(key) = event::read()?
                    && key.kind != KeyEventKind::Release
                {
                    match key.code {
                        KeyCode::Enter => text.push('\n'),
                        KeyCode::Tab => text.push('\t'),
                        KeyCode::Esc => text.push('\x1b'),
                        KeyCode::Char(c) => text.push(c),
                        _ => {}
                    }
                    if text.ends_with("\x1b[201~") {
                        text.truncate(text.len() - 6);
                        break;
                    }
                }
            }
            return Ok(Event::Paste(text));
        }
    }
    Ok(first)
}

pub fn clear() -> Result<()> {
    if interactive() {
        execute!(
            io::stdout(),
            Clear(ClearType::All),
            crossterm::cursor::MoveTo(0, 0)
        )?;
    }
    Ok(())
}
