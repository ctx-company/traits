//! Test-only process-global guards shared by app modules.

use std::ffi::OsString;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

// `CTX_CENTER_*` selects one process-global client endpoint. Tests that replace
// it with a protocol peer must serialize across all app modules, not merely
// within the module that happens to host the test.
static CENTER_ENV_LOCK: Mutex<()> = Mutex::new(());

pub(crate) fn center_environment_lock() -> MutexGuard<'static, ()> {
    CENTER_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A temporary center endpoint plus the process-global environment it needs.
/// Holding this value also holds the shared environment lock until restoration.
pub(crate) struct CenterPeer {
    root: PathBuf,
    listener: UnixListener,
    previous_env: Vec<(&'static str, Option<OsString>)>,
    _lock: MutexGuard<'static, ()>,
}

impl CenterPeer {
    pub(crate) fn install(label: &str) -> Self {
        let lock = center_environment_lock();
        let root = unique_temp_dir(label);
        let socket = root.join("center.sock");
        let listener = UnixListener::bind(&socket).expect("bind center peer");
        let center_env = [
            ("CTX_CENTER_SOCKET", socket.into_os_string()),
            ("CTX_CENTER_SPAWN_LOCK", root.join("center.lock").into()),
            ("CTX_CENTER_RUNS_ROOT", root.clone().into_os_string()),
            ("CTX_CENTER_INDEX", root.join("index.sqlite3").into()),
        ];
        let previous_env = center_env
            .iter()
            .map(|(name, _)| (*name, std::env::var_os(name)))
            .collect();
        unsafe {
            for (name, value) in &center_env {
                std::env::set_var(name, value);
            }
        }
        Self {
            root,
            listener,
            previous_env,
            _lock: lock,
        }
    }

    pub(crate) fn listener(&self) -> UnixListener {
        self.listener.try_clone().expect("clone center listener")
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }
}

/// Accept a client and complete the common center hello/ready exchange.
pub(crate) fn accept_center_client(listener: &UnixListener) -> UnixStream {
    let (mut stream, _) = listener.accept().expect("accept center client");
    complete_center_hello(&mut stream);
    stream
}

/// Complete the common center hello/ready exchange on an accepted stream.
pub(crate) fn complete_center_hello(stream: &mut UnixStream) {
    let mut hello = String::new();
    BufReader::new(stream.try_clone().expect("clone center stream"))
        .read_line(&mut hello)
        .expect("read center hello");
    let hello: serde_json::Value = serde_json::from_str(&hello).expect("decode center hello");
    let id = hello["id"].as_str().expect("center hello id");
    writeln!(
        stream,
        "{{\"kind\":\"ready\",\"id\":{}}}",
        serde_json::to_string(id).expect("encode center hello id")
    )
    .expect("write center ready");
}

/// Read one client request after the hello/ready exchange.
pub(crate) fn read_center_request(stream: &UnixStream) -> serde_json::Value {
    let mut request = String::new();
    BufReader::new(stream.try_clone().expect("clone center stream"))
        .read_line(&mut request)
        .expect("read center request");
    serde_json::from_str(&request).expect("decode center request")
}

/// Send a response whose payload is already expressed in the wire protocol.
pub(crate) fn write_center_response(
    stream: &mut UnixStream,
    request: &serde_json::Value,
    result: serde_json::Value,
) {
    writeln!(
        stream,
        "{{\"kind\":\"response\",\"id\":{},\"result\":{result}}}",
        serde_json::to_string(request["id"].as_str().expect("center request id"))
            .expect("encode center request id")
    )
    .expect("write center response");
}

/// Accept a subscription and emit its snapshot-start marker.
pub(crate) fn accept_center_subscription(listener: &UnixListener) -> UnixStream {
    let mut stream = accept_center_client(listener);
    let request = read_center_request(&stream);
    assert_eq!(
        request["kind"], "subscribe",
        "expected subscription request"
    );
    writeln!(
        stream,
        "{{\"kind\":\"snapshot-start\",\"id\":{}}}",
        serde_json::to_string(request["id"].as_str().expect("subscription request id"))
            .expect("encode subscription request id")
    )
    .expect("write snapshot start");
    stream
}

impl Drop for CenterPeer {
    fn drop(&mut self) {
        unsafe {
            for (name, previous) in self.previous_env.drain(..) {
                match previous {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn unique_temp_dir(label: &str) -> PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    loop {
        // Unix socket names have a small platform limit. `/tmp` plus this
        // compact, process-unique directory leaves room for `center.sock`.
        let root = PathBuf::from("/tmp").join(format!(
            "ctx-c-{label}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        match std::fs::create_dir(&root) {
            Ok(()) => return root,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => panic!("create center peer directory {}: {error}", root.display()),
        }
    }
}
