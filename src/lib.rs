//! A simple HTTP logger that sends any log to an HTTP-POST endpoint.
//!
//! Example Usage
//! ---
//! ```rust
//! fn main() {
//!     http_logger::init("http://localhost:8080".to_owned());
//!     log::info!("An Example Information!");
//! }
//! ```
//!
//! Example Request
//! ---
//! This is an example request that this crate will make when calling log::info!(...); with a HttpLogger:
//! ```http
//! POST / HTTP/1.1
//! Host: localhost:8080
//! Content-Length: 194
//! Content-Type: application/json; charset=UTF-8
//!
//! {"timestamp":{"secs_since_epoch":1772354249,"nanos_since_epoch":368051819},"level":"INFO","target":"example","module":"example","file":"src/main.rs","line":4,"message":"An Example Information!"}
//! ```
//! The [structure](./struct.LogEntry.html) in the body is public in this crate and can be used for deserialization on the server side.
//!
//! Example Server
//! ---
//! You can find an example (and very minimal) server implementation in the tests, but I highly advice one to use an async backend framework like actix-web or one of its many alternatives

use std::{sync::Mutex, time::SystemTime};

use log::{Level, LevelFilter, Log};
use minreq::ResponseLazy;
use serde::{Deserialize, Serialize};

/// Initializes the logger with the most verbose log-level possible.
pub fn init(endpoint: String) -> Result<(), log::SetLoggerError> {
    init_with_filter(endpoint, LevelFilter::max())
}

/// Initializes the logger with a log-level of your choice.
pub fn init_with_filter(endpoint: String, filter: LevelFilter) -> Result<(), log::SetLoggerError> {
    log::set_max_level(filter);
    log::set_boxed_logger(Box::new(HttpLogger::new(endpoint)))
}

/// The struct that implements Log, this can be initialized on its own for use with crates like multi_log.
pub struct HttpLogger {
    endpoint: String,
    reentrant_guard: Mutex<()>,
}

impl HttpLogger {
    /// Creates a new HttpLogger
    pub fn new(endpoint: String) -> Self {
        Self {
            endpoint,
            reentrant_guard: Mutex::new(()),
        }
    }
}

/// The struct that is serialized and send over as JSON. It is public so that users can deserialize it from its definition.
#[derive(Clone, Debug, Serialize, Deserialize, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct LogEntry {
    pub timestamp: SystemTime,
    pub level: Level,
    pub target: String,
    pub module: Option<String>,
    pub file: Option<String>,
    pub line: Option<u32>,
    pub message: Option<String>,
}

impl Log for HttpLogger {
    fn enabled(&self, _metadata: &log::Metadata) -> bool {
        true
    }

    fn log(&self, record: &log::Record) {
        // Since the libraries used in the following code do actively use log as well, we need to prevent them from causing infinite recursion.
        // It's a bit unfortunate that log only supports a single logger, libraries like spdlog would simply ignore this sink and use all the other ones.
        let _guard = match self.reentrant_guard.try_lock() {
            Ok(lock) => lock,
            Err(_) => return (),
        };

        let entry = LogEntry {
            timestamp: SystemTime::now(),
            level: record.level(),
            target: record.target().to_owned(),
            module: record.module_path().map(ToOwned::to_owned),
            file: record.file().map(ToOwned::to_owned),
            line: record.line(),
            message: record.args().as_str().map(ToOwned::to_owned),
        };

        let request = match minreq::post(&self.endpoint).with_json(&entry) {
            Ok(request) => request,
            Err(e) => {
                eprintln!("Error while serializing logs: {e}");
                return;
            }
        };

        let response = match request.send_lazy() {
            Ok(response) => response,
            Err(e) => {
                eprintln!("Error while sending logs to the endpoint: {e}");
                return;
            }
        };

        if !(200..300).contains(&response.status_code) {
            let status = response.reason_phrase.clone();

            // TODO: Remove once try_collect is not nightly
            fn try_collect(response: ResponseLazy) -> Result<Vec<u8>, minreq::Error> {
                let mut v = Vec::new();

                for b in response {
                    match b {
                        Ok(byte) => v.push(byte.0),
                        Err(e) => return Err(e),
                    }
                }

                Ok(v)
            }

            let bytes = try_collect(response);
            match bytes {
                Ok(bytes) if bytes.is_empty() => {
                    eprintln!("Log-Server returned non-successful status ({status})",)
                }
                Ok(bytes) => match String::from_utf8(bytes.clone()) {
                    Ok(s) => {
                        eprintln!(
                            "Log-Server returned non-successful status ({status}) and message: {s}",
                        )
                    }
                    Err(_) => eprintln!(
                        "Log-Server returned non-successful status ({status}) and byte message: {bytes:?}"
                    ),
                },
                Err(e) => eprintln!(
                    "Log-Server returned non-successful status ({status}), but couldn't read messag: {e}"
                ),
            }
        }
    }

    fn flush(&self) {}
}

#[cfg(test)]
mod tests {
    use std::{
        io::{BufRead, BufReader, Read, Write},
        net::TcpListener,
        sync::{Arc, Barrier, Mutex},
    };

    use log::LevelFilter;

    use crate::LogEntry;

    #[test]
    fn test() {
        let log_entries = Arc::new(Mutex::new(Vec::new()));
        let log_entries2 = log_entries.clone();

        let barrier = Arc::new(Barrier::new(2));
        let barrier2 = barrier.clone();

        std::thread::spawn(move || {
            let listener = TcpListener::bind("127.0.0.1:1111").unwrap();

            barrier2.wait();
            loop {
                let Ok((stream, _addr)) = listener.accept() else {
                    continue;
                };

                let mut bufreader = BufReader::new(stream);

                let mut buf = String::new();
                bufreader.read_line(&mut buf).unwrap();
                assert_eq!(buf, "POST /log HTTP/1.1\r\n");

                let mut size = 0usize;

                loop {
                    buf.clear();
                    bufreader.read_line(&mut buf).unwrap();
                    buf = buf.trim_end_matches(&['\r', '\n']).to_owned();
                    if buf.is_empty() {
                        break;
                    }

                    let Some((k, v)) = buf.split_once(": ") else {
                        continue;
                    };

                    if k.eq_ignore_ascii_case("Content-Length") {
                        size = v.parse::<usize>().unwrap();
                    }
                }

                let mut bytes = vec![0; size];
                bufreader.read_exact(&mut bytes).unwrap();

                let entry = serde_json::from_slice::<LogEntry>(&bytes).unwrap();

                log_entries2.lock().unwrap().push(entry);

                bufreader
                    .into_inner()
                    .write_all(b"HTTP/1.1 200 OK\r\n\r\n")
                    .unwrap();
            }
        });

        barrier.wait();

        crate::init_with_filter("http://127.0.0.1:1111/log".to_owned(), LevelFilter::Trace)
            .unwrap();

        log::error!("Test Error");
        log::warn!("Test Warn");
        log::info!("Test Info");
        log::debug!("Test Debug");
        log::trace!("Test Trace");

        let log_entries = log_entries.lock().unwrap();

        assert_eq!(log_entries.len(), 5);
        assert_eq!(log_entries[0].message.as_ref().unwrap(), "Test Error");
        assert_eq!(log_entries[1].message.as_ref().unwrap(), "Test Warn");
        assert_eq!(log_entries[2].message.as_ref().unwrap(), "Test Info");
        assert_eq!(log_entries[3].message.as_ref().unwrap(), "Test Debug");
        assert_eq!(log_entries[4].message.as_ref().unwrap(), "Test Trace");
    }
}
