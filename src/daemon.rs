//! skim-tab daemon — long-lived enrichment cache served over Unix socket.
//!
//! The interactive picker can't move into a daemon (skim needs the user's
//! TTY), so the daemon optimises the *enrichment* path: parses every YAML
//! spec once, holds the global `SpecRegistry` in memory, and answers
//! lookup queries from clients in microseconds.
//!
//! Wire format: line-delimited JSON, one request per connection.
//!
//! ```text
//! → {"kind":"ping"}
//! ← {"ok":true,"version":"0.1.0","spec_count":12}
//!
//! → {"kind":"lookup","command":"kubectl","word":"get"}
//! ← {"glyph":"◈","description":"Display resources"}
//!
//! → {"kind":"icon","command":"kubectl"}
//! ← {"icon":"⎈ "}
//!
//! → {"kind":"is_k8s","command":"kubectl"}
//! ← {"is_k8s":true}
//! ```
//!
//! On unknown `kind`, an `{"error":"..."}` envelope is returned and the
//! connection is closed. The client (`complete.rs` or any external tool)
//! is expected to fall back to fresh-exec semantics.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config;
use crate::specs::{DescriptionProvider, SpecRegistry};
use crate::{ANSI_DIM, ANSI_FROST, ANSI_RESET};

// ── Wire types ────────────────────────────────────────────────────────

/// A single request line. `kind` discriminates the variant; per-kind fields
/// are optional and only required for that specific operation.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub command: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub word: String,
    /// Used by `lookup_batch` and `prefetch` to ship many candidate words
    /// in one round-trip. Order is preserved in the response.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub words: Vec<String>,
}

/// One entry in a batched lookup response. `None` means no match.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct LookupHit {
    pub glyph: String,
    pub description: String,
}

/// Server response. Untagged so the client gets a flat object that's easy
/// to consume from `jq` / `jaq`. Errors are surfaced via the `error` field.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum Response {
    Pong {
        ok: bool,
        version: &'static str,
        spec_count: usize,
    },
    Lookup {
        glyph: Option<String>,
        description: Option<String>,
    },
    Icon {
        icon: Option<String>,
    },
    IsK8s {
        is_k8s: bool,
    },
    /// Batched: `hits[i]` is the lookup result for `request.words[i]`,
    /// or `null` if no spec matched.
    LookupBatch {
        hits: Vec<Option<LookupHit>>,
    },
    /// One round-trip that returns icon + is_k8s + batched word lookups
    /// for a single command. The hot path on every TAB.
    Prefetch {
        icon: Option<String>,
        is_k8s: bool,
        hits: Vec<Option<LookupHit>>,
    },
    Error {
        error: String,
    },
}

// ── Server ────────────────────────────────────────────────────────────

/// The daemon: holds the parsed spec registry behind an `Arc` so each
/// client thread can read concurrently without locking.
pub struct Server {
    provider: Arc<SpecRegistry>,
    spec_count: usize,
}

impl Server {
    /// Build a server with specs loaded from the supplied config (built-in
    /// + user dirs + project dir, same load order as the picker).
    pub fn new(specs_cfg: &config::SpecsConfig) -> Self {
        let provider = SpecRegistry::new(specs_cfg);
        let spec_count = provider.spec_count();
        Self {
            provider: Arc::new(provider),
            spec_count,
        }
    }

    /// Bind the Unix socket and serve forever. Spawns a thread per
    /// connection. Removes any stale socket file at the path before binding.
    pub fn serve(self, socket: &Path) -> Result<()> {
        if let Some(parent) = socket.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating socket parent dir: {}", parent.display()))?;
        }
        if socket.exists() {
            std::fs::remove_file(socket)
                .with_context(|| format!("removing stale socket: {}", socket.display()))?;
        }

        let listener = UnixListener::bind(socket)
            .with_context(|| format!("binding socket: {}", socket.display()))?;
        eprintln!(
            "{frost}\u{2744} skim-tab{reset} daemon listening on {dim}{path}{reset}  {dim}\u{00B7}{reset}  {frost}{specs}{reset} {dim}specs loaded{reset}",
            frost = ANSI_FROST,
            dim = ANSI_DIM,
            reset = ANSI_RESET,
            path = socket.display(),
            specs = self.spec_count,
        );

        let server = Arc::new(self);
        for stream in listener.incoming() {
            let stream = match stream {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("skim-tab daemon: accept failed: {e}");
                    continue;
                }
            };
            let server = Arc::clone(&server);
            thread::spawn(move || {
                if let Err(e) = handle_client(server.as_ref(), stream) {
                    eprintln!("skim-tab daemon: client error: {e}");
                }
            });
        }
        Ok(())
    }

    /// Synchronously answer a single request against this server's state.
    /// Exposed for direct in-process use (tests, future client refactor).
    pub fn answer(&self, req: &Request) -> Response {
        match req.kind.as_str() {
            "ping" => Response::Pong {
                ok: true,
                version: env!("CARGO_PKG_VERSION"),
                spec_count: self.spec_count,
            },
            "lookup" => {
                let hit = self.provider.lookup(&req.command, &req.word);
                Response::Lookup {
                    glyph: hit.map(|(g, _)| g.to_string()),
                    description: hit.map(|(_, d)| d.to_string()),
                }
            }
            "lookup_batch" => Response::LookupBatch {
                hits: req
                    .words
                    .iter()
                    .map(|w| {
                        self.provider.lookup(&req.command, w).map(|(g, d)| LookupHit {
                            glyph: g.to_string(),
                            description: d.to_string(),
                        })
                    })
                    .collect(),
            },
            "prefetch" => Response::Prefetch {
                icon: self.provider.icon(&req.command).map(String::from),
                is_k8s: self.provider.is_k8s_command(&req.command),
                hits: req
                    .words
                    .iter()
                    .map(|w| {
                        self.provider.lookup(&req.command, w).map(|(g, d)| LookupHit {
                            glyph: g.to_string(),
                            description: d.to_string(),
                        })
                    })
                    .collect(),
            },
            "icon" => Response::Icon {
                icon: self.provider.icon(&req.command).map(String::from),
            },
            "is_k8s" => Response::IsK8s {
                is_k8s: self.provider.is_k8s_command(&req.command),
            },
            other => Response::Error {
                error: format!("unknown kind: {other}"),
            },
        }
    }
}

fn handle_client(server: &Server, stream: UnixStream) -> Result<()> {
    let mut reader = BufReader::new(stream.try_clone().context("clone unix stream")?);
    let mut line = String::new();
    reader.read_line(&mut line).context("read request line")?;
    if line.is_empty() {
        return Ok(());
    }

    let response = match serde_json::from_str::<Request>(line.trim_end()) {
        Ok(req) => server.answer(&req),
        Err(e) => Response::Error {
            error: format!("parse: {e}"),
        },
    };

    let mut out = stream;
    let payload = serde_json::to_string(&response).context("serialize response")?;
    writeln!(out, "{payload}").context("write response")?;
    Ok(())
}

// ── Client helpers (for `skim-tab daemon-status` etc.) ────────────────

/// Default socket path: `$XDG_RUNTIME_DIR/skim-tab.socket` if set, otherwise
/// `$TMPDIR/skim-tab.<user>.socket`, otherwise `/tmp/skim-tab.<user>.socket`.
pub fn default_socket() -> PathBuf {
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        return PathBuf::from(dir).join("skim-tab.socket");
    }
    let user = std::env::var("USER").unwrap_or_else(|_| "default".to_string());
    let tmp = std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".to_string());
    // Strip trailing slash from TMPDIR (macOS sets it with one)
    let tmp = tmp.trim_end_matches('/');
    PathBuf::from(tmp).join(format!("skim-tab.{user}.socket"))
}

/// Send a single request to the daemon and read one line of response.
/// Used by the `daemon-status` subcommand and tests.
pub fn query(socket: &Path, req: &Request) -> Result<String> {
    use std::io::Read;
    let mut stream =
        UnixStream::connect(socket).with_context(|| format!("connect: {}", socket.display()))?;
    let payload = serde_json::to_string(req).context("serialize request")?;
    writeln!(stream, "{payload}").context("write request")?;
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .context("read response")?;
    Ok(response.trim_end().to_string())
}

// ── Cached client-side provider ────────────────────────────────────────

/// `DescriptionProvider` backed by data fetched from the daemon and held
/// in owned heap data. Built once per TAB via [`prefetch`], then queried
/// many times during candidate enrichment with no further I/O.
///
/// Returned `&str` references are valid for the lifetime of the
/// `CachedProvider` itself — the trait's `&self` lifetime is enough.
pub struct CachedProvider {
    command: String,
    icon: Option<String>,
    is_k8s: bool,
    descriptions: HashMap<String, (String, String)>,
}

impl CachedProvider {
    /// Number of cached (command, word) lookups.
    pub fn description_count(&self) -> usize {
        self.descriptions.len()
    }
}

impl DescriptionProvider for CachedProvider {
    fn lookup(&self, command: &str, word: &str) -> Option<(&str, &str)> {
        if command != self.command {
            return None;
        }
        self.descriptions
            .get(word)
            .map(|(g, d)| (g.as_str(), d.as_str()))
    }

    fn icon(&self, command: &str) -> Option<&str> {
        if command == self.command {
            self.icon.as_deref()
        } else {
            None
        }
    }

    fn is_k8s_command(&self, command: &str) -> bool {
        command == self.command && self.is_k8s
    }
}

/// One round-trip that hydrates a `CachedProvider` for `command` containing
/// every candidate word the picker is about to display. After this call,
/// downstream `lookup`/`icon`/`is_k8s_command` queries hit pure RAM.
pub fn prefetch(socket: &Path, command: &str, words: &[&str]) -> Result<CachedProvider> {
    let req = Request {
        kind: "prefetch".to_string(),
        command: command.to_string(),
        word: String::new(),
        words: words.iter().map(|s| s.to_string()).collect(),
    };
    let raw = query(socket, &req)?;
    let v: serde_json::Value = serde_json::from_str(&raw).context("parse prefetch response")?;

    if let Some(err) = v.get("error").and_then(|e| e.as_str()) {
        anyhow::bail!("daemon error: {err}");
    }

    let icon = v
        .get("icon")
        .and_then(|i| i.as_str())
        .map(|s| s.to_string());
    let is_k8s = v.get("is_k8s").and_then(|b| b.as_bool()).unwrap_or(false);

    let mut descriptions = HashMap::new();
    if let Some(hits) = v.get("hits").and_then(|h| h.as_array()) {
        for (word, hit) in words.iter().zip(hits.iter()) {
            if let Some(obj) = hit.as_object() {
                if let (Some(glyph), Some(desc)) = (
                    obj.get("glyph").and_then(|g| g.as_str()),
                    obj.get("description").and_then(|d| d.as_str()),
                ) {
                    descriptions.insert(
                        (*word).to_string(),
                        (glyph.to_string(), desc.to_string()),
                    );
                }
            }
        }
    }

    Ok(CachedProvider {
        command: command.to_string(),
        icon,
        is_k8s,
        descriptions,
    })
}

/// Resolve the daemon socket from `SKIM_TAB_DAEMON_SOCKET`, falling back to
/// [`default_socket`]. Returns `None` if the resolved path doesn't exist —
/// the client should then use the local `SpecRegistry` instead.
pub fn socket_if_alive() -> Option<PathBuf> {
    let path = std::env::var("SKIM_TAB_DAEMON_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|_| default_socket());
    if path.exists() { Some(path) } else { None }
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn test_server() -> Server {
        Server::new(&config::SpecsConfig::default())
    }

    #[test]
    fn ping_returns_version_and_spec_count() {
        let s = test_server();
        let resp = s.answer(&Request {
            kind: "ping".into(),
            ..Default::default()
        });
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["ok"], true);
        assert!(json["version"].is_string());
        assert!(json["spec_count"].as_u64().unwrap() >= 12);
    }

    #[test]
    fn lookup_known_subcommand_returns_glyph_and_description() {
        let s = test_server();
        let resp = s.answer(&Request {
            kind: "lookup".into(),
            command: "kubectl".into(),
            word: "get".into(),
            ..Default::default()
        });
        let json = serde_json::to_value(&resp).unwrap();
        assert!(json["description"].is_string());
        assert!(json["glyph"].is_string());
    }

    #[test]
    fn lookup_unknown_subcommand_returns_nulls() {
        let s = test_server();
        let resp = s.answer(&Request {
            kind: "lookup".into(),
            command: "kubectl".into(),
            word: "totally-not-a-real-subcommand".into(),
            ..Default::default()
        });
        let json = serde_json::to_value(&resp).unwrap();
        assert!(json["description"].is_null());
        assert!(json["glyph"].is_null());
    }

    #[test]
    fn icon_returns_some_for_known_command() {
        let s = test_server();
        let resp = s.answer(&Request {
            kind: "icon".into(),
            command: "kubectl".into(),
            ..Default::default()
        });
        let json = serde_json::to_value(&resp).unwrap();
        assert!(json["icon"].is_string());
    }

    #[test]
    fn is_k8s_marks_kubectl_as_kubernetes() {
        let s = test_server();
        let resp = s.answer(&Request {
            kind: "is_k8s".into(),
            command: "kubectl".into(),
            ..Default::default()
        });
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["is_k8s"], true);
    }

    #[test]
    fn is_k8s_marks_aws_as_not_kubernetes() {
        let s = test_server();
        let resp = s.answer(&Request {
            kind: "is_k8s".into(),
            command: "aws".into(),
            ..Default::default()
        });
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["is_k8s"], false);
    }

    #[test]
    fn unknown_kind_returns_error() {
        let s = test_server();
        let resp = s.answer(&Request {
            kind: "no-such-kind".into(),
            ..Default::default()
        });
        let json = serde_json::to_value(&resp).unwrap();
        assert!(json["error"].as_str().unwrap().contains("unknown kind"));
    }

    fn start_test_daemon() -> (tempfile::TempDir, PathBuf) {
        use std::time::Duration;
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("skim-tab.socket");
        let socket_for_thread = socket.clone();
        std::thread::spawn(move || {
            let s = Server::new(&config::SpecsConfig::default());
            let _ = s.serve(&socket_for_thread);
        });
        for _ in 0..20 {
            if socket.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(socket.exists(), "socket never appeared");
        (dir, socket)
    }

    #[test]
    fn end_to_end_socket_roundtrip() {
        let (_dir, socket) = start_test_daemon();
        let resp = query(
            &socket,
            &Request {
                kind: "ping".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let json: serde_json::Value = serde_json::from_str(&resp).unwrap();
        assert_eq!(json["ok"], true);
    }

    #[test]
    fn lookup_batch_returns_one_hit_per_word_in_order() {
        let s = test_server();
        let resp = s.answer(&Request {
            kind: "lookup_batch".into(),
            command: "kubectl".into(),
            word: String::new(),
            words: vec![
                "get".into(),
                "totally-unknown".into(),
                "apply".into(),
            ],
        });
        let json = serde_json::to_value(&resp).unwrap();
        let hits = json["hits"].as_array().unwrap();
        assert_eq!(hits.len(), 3);
        assert!(hits[0].is_object(), "get should be a hit");
        assert!(hits[1].is_null(), "unknown word should be null");
        assert!(hits[2].is_object(), "apply should be a hit");
    }

    #[test]
    fn prefetch_returns_icon_is_k8s_and_batch_in_one_response() {
        let s = test_server();
        let resp = s.answer(&Request {
            kind: "prefetch".into(),
            command: "kubectl".into(),
            word: String::new(),
            words: vec!["get".into(), "apply".into()],
        });
        let json = serde_json::to_value(&resp).unwrap();
        assert!(json["icon"].is_string());
        assert_eq!(json["is_k8s"], true);
        assert_eq!(json["hits"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn cached_provider_serves_lookups_from_local_data() {
        let (_dir, socket) = start_test_daemon();
        let provider = prefetch(&socket, "kubectl", &["get", "apply", "totally-unknown"])
            .expect("prefetch failed");

        // Hit
        let (glyph, desc) = provider
            .lookup("kubectl", "get")
            .expect("get should be cached");
        assert!(!glyph.is_empty(), "glyph should be populated");
        assert!(!desc.is_empty(), "description should be populated");

        // Miss inside same command
        assert!(provider.lookup("kubectl", "totally-unknown").is_none());

        // Miss for different command (cache is single-command)
        assert!(provider.lookup("git", "get").is_none());

        // Icon + is_k8s carried across
        assert!(provider.icon("kubectl").is_some());
        assert!(provider.is_k8s_command("kubectl"));
        assert!(!provider.is_k8s_command("git"));
    }

    #[test]
    fn socket_if_alive_returns_none_when_path_missing() {
        let dir = tempfile::tempdir().unwrap();
        let bogus = dir.path().join("does-not-exist.socket");
        let original = std::env::var("SKIM_TAB_DAEMON_SOCKET").ok();
        unsafe { std::env::set_var("SKIM_TAB_DAEMON_SOCKET", &bogus); }
        assert!(socket_if_alive().is_none());
        unsafe {
            match original {
                Some(v) => std::env::set_var("SKIM_TAB_DAEMON_SOCKET", v),
                None => std::env::remove_var("SKIM_TAB_DAEMON_SOCKET"),
            }
        }
    }

    #[test]
    fn socket_if_alive_returns_some_when_socket_exists() {
        let (_dir, socket) = start_test_daemon();
        let original = std::env::var("SKIM_TAB_DAEMON_SOCKET").ok();
        unsafe { std::env::set_var("SKIM_TAB_DAEMON_SOCKET", &socket); }
        assert_eq!(socket_if_alive(), Some(socket));
        unsafe {
            match original {
                Some(v) => std::env::set_var("SKIM_TAB_DAEMON_SOCKET", v),
                None => std::env::remove_var("SKIM_TAB_DAEMON_SOCKET"),
            }
        }
    }

    #[test]
    fn default_socket_uses_xdg_runtime_dir_when_set() {
        let dir = tempfile::tempdir().unwrap();
        let original = std::env::var("XDG_RUNTIME_DIR").ok();
        // SAFETY: env mutation; restore in finally-style block at the end.
        unsafe { std::env::set_var("XDG_RUNTIME_DIR", dir.path()); }
        let path = default_socket();
        assert_eq!(path, dir.path().join("skim-tab.socket"));
        unsafe {
            match original {
                Some(v) => std::env::set_var("XDG_RUNTIME_DIR", v),
                None => std::env::remove_var("XDG_RUNTIME_DIR"),
            }
        }
    }
}
