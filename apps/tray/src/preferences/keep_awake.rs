//! Sleep inhibition belongs to one long-lived thread (required by Windows). A synchronous
//! acknowledgement makes disabling and App exit release the assertion before returning.
use std::sync::{Mutex, mpsc};
use std::thread::{self, JoinHandle};

pub struct Guard {
    worker: Mutex<Option<Worker>>,
}

struct Worker {
    requests: mpsc::Sender<(bool, mpsc::Sender<Result<(), String>>)>,
    thread: JoinHandle<()>,
}

impl Default for Guard {
    fn default() -> Self {
        let (requests, receiver) = mpsc::channel::<(bool, mpsc::Sender<Result<(), String>>)>();
        let thread = thread::spawn(move || {
            let mut inhibitor = Inhibitor::default();
            while let Ok((enabled, reply)) = receiver.recv() {
                let _ = reply.send(inhibitor.set(enabled));
            }
            let _ = inhibitor.set(false);
        });
        Self {
            worker: Mutex::new(Some(Worker { requests, thread })),
        }
    }
}

impl Guard {
    pub fn set(&self, enabled: bool) -> Result<(), String> {
        let worker = self.worker.lock().unwrap();
        let Some(worker) = worker.as_ref() else {
            return Ok(());
        };
        let (reply, receiver) = mpsc::channel();
        worker
            .requests
            .send((enabled, reply))
            .map_err(|error| error.to_string())?;
        receiver.recv().map_err(|error| error.to_string())?
    }

    pub fn shutdown(&self) {
        if let Some(worker) = self.worker.lock().unwrap().take() {
            drop(worker.requests);
            let _ = worker.thread.join();
        }
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(target_os = "macos")]
#[derive(Default)]
struct Inhibitor {
    child: Option<std::process::Child>,
}

#[cfg(target_os = "macos")]
impl Inhibitor {
    fn set(&mut self, enabled: bool) -> Result<(), String> {
        if let Some(child) = self.child.as_mut()
            && child
                .try_wait()
                .map_err(|error| error.to_string())?
                .is_some()
        {
            self.child.take();
        }
        if enabled && self.child.is_none() {
            // -i blocks idle system sleep, never display sleep. -w also releases on parent death.
            let mut command = std::process::Command::new("/usr/bin/caffeinate");
            command
                .args(["-i", "-w", &std::process::id().to_string()])
                .current_dir(crate::temp_dir::usable())
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
            crate::temp_dir::apply_to_command(&mut command);
            self.child = Some(command.spawn().map_err(|error| error.to_string())?);
        } else if !enabled && let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        Ok(())
    }
}

#[cfg(windows)]
#[derive(Default)]
struct Inhibitor {
    enabled: bool,
}

#[cfg(windows)]
impl Inhibitor {
    fn set(&mut self, enabled: bool) -> Result<(), String> {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn SetThreadExecutionState(flags: u32) -> u32;
        }
        if enabled != self.enabled {
            // SAFETY: called and released on this same dedicated thread, no pointer arguments.
            if unsafe { SetThreadExecutionState(0x80000000 | if enabled { 1 } else { 0 }) } == 0 {
                return Err(std::io::Error::last_os_error().to_string());
            }
            self.enabled = enabled;
        }
        Ok(())
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
#[derive(Default)]
struct Inhibitor;

#[cfg(not(any(target_os = "macos", windows)))]
impl Inhibitor {
    fn set(&mut self, enabled: bool) -> Result<(), String> {
        if enabled {
            Err("此系统不支持阻止睡眠".into())
        } else {
            Ok(())
        }
    }
}
