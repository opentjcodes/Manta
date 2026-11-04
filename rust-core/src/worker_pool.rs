use std::net::ToSocketAddrs;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

type Job = Box<dyn FnOnce() + Send + 'static>;

/// Multi-Core Rust Worker Pool (`worker_pool.rs`):
/// Distributes DNS pre-warming, background tab compression, and stream tasks across
/// multiple CPU hardware threads while keeping idle CPU/battery usage at 0% via channel park.
pub struct MantaWorkerPool {
    sender: Sender<Job>,
    core_count: usize,
}

impl MantaWorkerPool {
    pub fn new(requested_cores: usize) -> Self {
        let detected = thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .clamp(2, 8);
        let core_count = if requested_cores == 0 {
            detected
        } else {
            requested_cores.clamp(1, 8)
        };

        let (sender, receiver) = mpsc::channel::<Job>();
        let shared_rx = Arc::new(Mutex::new(receiver));

        for idx in 0..core_count {
            let rx_clone = Arc::clone(&shared_rx);
            let _ = thread::Builder::new()
                .name(format!("manta-core-worker-{}", idx))
                .spawn(move || loop {
                    let job_opt = {
                        let Ok(lock) = rx_clone.lock() else {
                            break;
                        };
                        lock.recv().ok()
                    };
                    match job_opt {
                        Some(job) => job(),
                        None => break,
                    }
                });
        }

        Self { sender, core_count }
    }

    #[inline]
    pub fn core_count(&self) -> usize {
        self.core_count
    }

    pub fn spawn_task<F>(&self, task: F)
    where
        F: FnOnce() + Send + 'static,
    {
        let _ = self.sender.send(Box::new(task));
    }

    pub fn prewarm_url_dns(&self, url: &str) {
        let host = extract_host_for_dns(url).to_string();
        if host.is_empty() || host.contains("localhost") {
            return;
        }
        self.spawn_task(move || {
            let addr = format!("{}:443", host);
            let _ = addr.to_socket_addrs();
        });
    }
}

fn extract_host_for_dns(url: &str) -> &str {
    let without_scheme = match url.find("://") {
        Some(idx) => &url[idx + 3..],
        None => url,
    };
    let end = without_scheme
        .find(['/', ':', '?', '#'])
        .unwrap_or(without_scheme.len());
    &without_scheme[..end]
}
