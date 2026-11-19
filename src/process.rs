use anyhow::{Context, Result};
use std::io::{self, Read};
use std::path::Path;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

#[cfg(windows)]
struct Job(windows_sys::Win32::Foundation::HANDLE);
#[cfg(windows)]
impl Job {
    fn new() -> io::Result<Self> {
        use windows_sys::Win32::System::JobObjects::*;
        // The handle is owned by Job, is not inherited and is closed exactly once.
        unsafe {
            let h = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if h.is_null() {
                return Err(io::Error::last_os_error());
            }
            let job = Self(h);
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                h,
                JobObjectExtendedLimitInformation,
                (&raw const info).cast(),
                std::mem::size_of_val(&info) as u32,
            ) == 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(job)
        }
    }
}
#[cfg(windows)]
impl Drop for Job {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

pub struct Process {
    pub child: Child,
    #[cfg(windows)]
    _job: Job,
}
impl Process {
    pub fn start(cmd: &mut Command) -> Result<Self> {
        #[cfg(windows)]
        {
            use std::os::windows::{io::AsRawHandle, process::CommandExt};
            use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;
            let job = Job::new()?;
            let mut child = cmd.creation_flags(0x08000000).spawn()?;
            if unsafe { AssignProcessToJobObject(job.0, child.as_raw_handle()) } == 0 {
                let err = io::Error::last_os_error();
                let _ = child.kill();
                let _ = child.wait();
                return Err(err.into());
            }
            Ok(Self { child, _job: job })
        }
        #[cfg(not(windows))]
        {
            Ok(Self {
                child: cmd.spawn()?,
            })
        }
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn drain(mut pipe: impl Read) -> io::Result<Vec<u8>> {
    let mut saved = Vec::new();
    let mut buf = [0; 8192];
    loop {
        let n = pipe.read(&mut buf)?;
        if n == 0 {
            break;
        }
        saved.extend_from_slice(&buf[..n.min(16_001 - saved.len())]);
    }
    Ok(saved)
}
pub fn shell(root: &Path, script: &str, timeout: u64) -> Result<String> {
    use std::process::Stdio;
    let system = std::env::var_os("SystemRoot").context("Windows system directory unavailable")?;
    let mut cmd =
        Command::new(Path::new(&system).join("System32/WindowsPowerShell/v1.0/powershell.exe"));
    cmd.args([
        "-NoLogo",
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        script,
    ])
    .current_dir(root)
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    let mut proc = Process::start(&mut cmd).context("Could not start PowerShell")?;
    let stdout = proc.child.stdout.take().unwrap();
    let stderr = proc.child.stderr.take().unwrap();
    let out = std::thread::spawn(move || drain(stdout));
    let err = std::thread::spawn(move || drain(stderr));
    let until = Instant::now() + Duration::from_secs(timeout);
    let status = loop {
        if crate::interrupted() {
            break "Interrupted".to_owned();
        }
        if Instant::now() > until {
            break format!("Timed out after {timeout}s");
        }
        if let Some(status) = proc.child.try_wait()? {
            break format!("Exit code: {}", status.code().unwrap_or(-1));
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    drop(proc);
    let mut text = status;
    for reader in [out, err] {
        let data = reader
            .join()
            .map_err(|_| anyhow::anyhow!("Output reader stopped"))??;
        if !data.is_empty() {
            text.push('\n');
            text.push_str(&String::from_utf8_lossy(&data[..data.len().min(16_000)]));
            if data.len() > 16_000 {
                text.push_str("\n[output capped]");
            }
        }
    }
    Ok(text)
}
